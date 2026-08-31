//! 文字起こしのアダプタ。whisper.cpp（`whisper-cli`）を使う。
//!
//! # ffmpeg を挟む理由
//!
//! `whisper-cli` は **WAV しか読めない**（実測。m4a を渡すと
//! `read_audio_data: failed to read audio data`）。取得したものは mp4 や m4a なので、
//! 16kHz モノラルの PCM へ落としてから渡す。
//!
//! #5 は「`ffmpeg` は yt-dlp が内部で呼ぶ。escrow が直接叩くことはない」としているが、
//! ここだけは escrow が直接叩く。
//!
//! この変換が [`Transcribe`] の内側に閉じているのは、口が「手元の実体を文字起こしする」
//! だけを約束しているため。WAV を要求しない別の実装へ替えれば、変換ごと消える。

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use super::invocation::{Invocation, run};
use super::{AdapterError, Transcribe};
use crate::asset::{Asset, AssetKind};
use crate::config::Language;

const WHISPER: &str = "whisper-cli";
const FFMPEG: &str = "ffmpeg";

/// 文字起こしが書き出す形式。#1 の `transcript.<ordinal>.vtt` と合う。
const EXTENSION: &str = "vtt";

pub struct Whisper {
    whisper: PathBuf,
    ffmpeg: PathBuf,
    model: PathBuf,
    language: Language,
}

impl Whisper {
    pub fn new(
        whisper: impl Into<PathBuf>,
        ffmpeg: impl Into<PathBuf>,
        model: impl Into<PathBuf>,
        language: Language,
    ) -> Self {
        Self {
            whisper: whisper.into(),
            ffmpeg: ffmpeg.into(),
            model: model.into(),
            language,
        }
    }
}

// ------------------------------------------------------------ 引数の組み立て

/// 取得したものを、文字起こしが読める形へ落とす。
///
/// 16kHz モノラルの PCM。映像は捨てる。
pub fn convert_argv(ffmpeg: &Path, input: &Path, output: &Path) -> Invocation {
    Invocation::new(ffmpeg)
        // 端末を持たないので、上書きの問い合わせで止まらせない。
        .arg("-nostdin")
        .args(["-loglevel", "error"])
        .arg("-i")
        .arg(input)
        .arg("-vn")
        .args(["-ar", "16000"])
        .args(["-ac", "1"])
        .args(["-c:a", "pcm_s16le"])
        .arg("-y")
        .arg(output)
}

/// 文字起こし本体。
///
/// `-m` と `-l` は必ず渡す。既定がそれぞれ相対パスのモデルと `en` なので、
/// 省くと動かないか、言語を取り違える（#5）。
///
/// `-of` は拡張子を除いた出力先。`whisper-cli` が `.vtt` を足す。
pub fn transcribe_argv(
    whisper: &Path,
    model: &Path,
    language: &Language,
    input: &Path,
    output_stem: &Path,
) -> Invocation {
    Invocation::new(whisper)
        .arg("-m")
        .arg(model)
        .arg("-l")
        .arg(language.to_string())
        .arg("-ovtt")
        // 結果以外を出させない。読み取る側が余計な行を見なくて済む。
        .arg("-np")
        .arg("-of")
        .arg(output_stem)
        .arg(input)
}

/// 文字起こしが書くファイルの名前。
pub fn transcript_asset(ordinal: NonZeroU32) -> Asset {
    Asset::new(AssetKind::Transcript, ordinal, EXTENSION)
}

// ------------------------------------------------------------------ 実行

impl Transcribe for Whisper {
    async fn transcribe(
        &self,
        media: &Path,
        into: &Path,
        ordinal: NonZeroU32,
    ) -> Result<Asset, AdapterError> {
        let asset = transcript_asset(ordinal);

        // 変換したものは残さない。落ちても消える場所へ置く。
        let scratch = tempfile::tempdir().map_err(|source| AdapterError::Launch {
            program: FFMPEG.to_owned(),
            source,
        })?;
        let wav = scratch.path().join("audio.wav");

        let converted = run(&convert_argv(&self.ffmpeg, media, &wav), None).await?;
        if !converted.success {
            return Err(AdapterError::Transient {
                program: FFMPEG.to_owned(),
                detail: converted.stderr_tail(),
            });
        }

        // `-of` は拡張子を除いた形を取るので、`transcript.1.vtt` から `.vtt` を落とす。
        let stem = into.join(format!("{}.{ordinal}", AssetKind::Transcript));
        let transcribed = run(
            &transcribe_argv(&self.whisper, &self.model, &self.language, &wav, &stem),
            None,
        )
        .await?;

        if !transcribed.success {
            return Err(AdapterError::Transient {
                program: WHISPER.to_owned(),
                detail: transcribed.stderr_tail(),
            });
        }

        // 成功と言われても、書かれていなければ文字起こしは無い。
        let written = into.join(asset.file_name());
        if !written.is_file() {
            return Err(AdapterError::Parse {
                program: WHISPER.to_owned(),
                detail: format!("成功したが {} が無い", asset.file_name()),
            });
        }
        Ok(asset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ordinal(n: u32) -> NonZeroU32 {
        NonZeroU32::new(n).unwrap()
    }

    // ---- 引数の組み立て。プロセスは起動しない ----

    #[test]
    fn the_conversion_asks_for_what_the_transcriber_can_read() {
        let invocation = convert_argv(
            Path::new("/opt/homebrew/bin/ffmpeg"),
            Path::new("/m/42/video.1.mp4"),
            Path::new("/tmp/audio.wav"),
        );
        let args = invocation.args_as_str().unwrap();

        // whisper-cli は 16kHz モノラルの PCM しか読めない。
        assert!(args.windows(2).any(|w| w == ["-ar", "16000"]));
        assert!(args.windows(2).any(|w| w == ["-ac", "1"]));
        assert!(args.windows(2).any(|w| w == ["-c:a", "pcm_s16le"]));
        // 映像は要らない。
        assert!(args.contains(&"-vn"));
    }

    /// `-m` と `-l` を省くと、既定が相対パスのモデルと `en` なので動かないか
    /// 言語を取り違える（#5）。
    #[test]
    fn the_model_and_the_language_are_always_passed() {
        let invocation = transcribe_argv(
            Path::new("/opt/homebrew/bin/whisper-cli"),
            Path::new("/models/ggml-large-v3-turbo.bin"),
            &Language::Code("ja".to_owned()),
            Path::new("/tmp/audio.wav"),
            Path::new("/m/42/transcript.1"),
        );
        let args = invocation.args_as_str().unwrap();

        assert!(
            args.windows(2)
                .any(|w| w == ["-m", "/models/ggml-large-v3-turbo.bin"])
        );
        assert!(args.windows(2).any(|w| w == ["-l", "ja"]));
        assert!(args.contains(&"-ovtt"));
        // `-of` は拡張子を除いた形。whisper-cli が `.vtt` を足す。
        assert!(args.windows(2).any(|w| w == ["-of", "/m/42/transcript.1"]));
    }

    /// 自動判定は設定の [`Language::Auto`] が決め、綴りはこのアダプタが知る。
    #[test]
    fn auto_detection_passes_the_transcribers_own_word() {
        let invocation = transcribe_argv(
            Path::new("whisper-cli"),
            Path::new("/m.bin"),
            &Language::Auto,
            Path::new("/a.wav"),
            Path::new("/t"),
        );

        assert!(
            invocation
                .args_as_str()
                .unwrap()
                .windows(2)
                .any(|w| w == ["-l", "auto"])
        );
    }

    #[test]
    fn the_output_follows_the_naming_rule() {
        assert_eq!(transcript_asset(ordinal(1)).file_name(), "transcript.1.vtt");
        // 断片ごとに1本（#1）。
        assert_eq!(transcript_asset(ordinal(3)).file_name(), "transcript.3.vtt");
    }

    // ---- 実物で通すところまで ----

    /// ffmpeg と whisper-cli が揃っているときだけ走る。
    ///
    /// 無音1秒なので中身は空でよく、**2つのツールが繋がること**だけを見る。
    #[tokio::test]
    async fn transcribes_a_real_file_end_to_end() {
        let Some(model) = model_for_tests() else {
            eprintln!("モデルが無いので飛ばす");
            return;
        };
        let (Some(ffmpeg), Some(whisper)) = (find("ffmpeg"), find("whisper-cli")) else {
            eprintln!("ツールが無いので飛ばす");
            return;
        };

        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("video.1.m4a");
        let silence = run(
            &Invocation::new(&ffmpeg)
                .args(["-nostdin", "-loglevel", "error"])
                .args(["-f", "lavfi", "-i", "anullsrc=r=44100:cl=stereo"])
                .args(["-t", "1", "-c:a", "aac", "-y"])
                .arg(&media),
            None,
        )
        .await
        .unwrap();
        assert!(silence.success, "テスト用の音声を作れない");

        let whisper = Whisper::new(whisper, ffmpeg, model, Language::Code("ja".to_owned()));
        let asset = whisper
            .transcribe(&media, dir.path(), ordinal(1))
            .await
            .expect("文字起こしが通ること");

        assert_eq!(asset.file_name(), "transcript.1.vtt");
        let written = std::fs::read_to_string(dir.path().join(asset.file_name())).unwrap();
        assert!(written.starts_with("WEBVTT"), "VTT として書かれていること");
    }

    fn find(program: &str) -> Option<PathBuf> {
        which::which(program).ok()
    }

    fn model_for_tests() -> Option<PathBuf> {
        let path =
            std::env::home_dir()?.join(".local/share/whisper-models/ggml-large-v3-turbo.bin");
        path.is_file().then_some(path)
    }
}
