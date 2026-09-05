//! 外部プロセスの呼び出しを値にする。
//!
//! **組み立てと実行を分けるための型。** 引数の並びを [`Invocation`] という値にして
//! おくと、フラグが正しいかをプロセスを起動せずに確かめられる。ツールのフラグが
//! 変わったとき、落ちるのは組み立ての側だけになる。

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::AdapterError;

/// 1回ぶんの呼び出し。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

impl Invocation {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
        }
    }

    #[must_use]
    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    #[must_use]
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|a| a.as_ref().to_owned()));
        self
    }

    /// テストで突き合わせるための形。引数が UTF-8 でなければ `None`。
    pub fn args_as_str(&self) -> Option<Vec<&str>> {
        self.args.iter().map(|a| a.to_str()).collect()
    }

    /// エラーメッセージに出す名前。
    pub fn program_name(&self) -> String {
        self.program
            .file_name()
            .unwrap_or(self.program.as_os_str())
            .to_string_lossy()
            .into_owned()
    }
}

/// 走り終えたプロセス。
#[derive(Debug, Clone)]
pub struct Completed {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Completed {
    /// 標準エラーの末尾。エラーメッセージへ載せる用。
    pub fn stderr_tail(&self) -> String {
        const LIMIT: usize = 400;
        let trimmed = self.stderr.trim();

        match trimmed.char_indices().nth_back(LIMIT) {
            Some((at, _)) => format!("…{}", &trimmed[at..]),
            None => trimmed.to_owned(),
        }
    }
}

/// 走らせて、出力を丸ごと受け取る。
///
/// 検知・生存確認・文字起こしのように**終わりがある**呼び出し向け。ライブの録画は
/// 数時間走って途中で止める必要があるので、そちらはエンジンが別に扱う（#7 Phase 6）。
pub async fn run(
    invocation: &Invocation,
    working_dir: Option<&Path>,
) -> Result<Completed, AdapterError> {
    let mut command = tokio::process::Command::new(&invocation.program);
    command.args(&invocation.args);
    if let Some(dir) = working_dir {
        command.current_dir(dir);
    }
    // 端末を持たない場所から動くので、対話的な問い合わせで止まらせない。
    command.stdin(std::process::Stdio::null());

    let output = command
        .output()
        .await
        .map_err(|source| AdapterError::Launch {
            program: invocation.program_name(),
            source,
        })?;

    Ok(Completed {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_an_argument_list_without_running_anything() {
        let invocation = Invocation::new("/opt/homebrew/bin/yt-dlp")
            .arg("--simulate")
            .args(["--print", "%(availability)s"])
            .arg("https://www.youtube.com/watch?v=dQw4w9WgXcQ");

        assert_eq!(invocation.program_name(), "yt-dlp");
        assert_eq!(
            invocation.args_as_str().unwrap(),
            [
                "--simulate",
                "--print",
                "%(availability)s",
                "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
            ]
        );
    }

    #[tokio::test]
    async fn reports_a_missing_program_as_a_launch_failure() {
        let invocation = Invocation::new("/nonexistent/tool");

        assert!(matches!(
            run(&invocation, None).await,
            Err(AdapterError::Launch { .. })
        ));
    }

    #[tokio::test]
    async fn captures_both_streams_and_the_outcome() {
        let ok = run(
            &Invocation::new("/bin/sh").args(["-c", "echo out; echo err >&2"]),
            None,
        )
        .await
        .unwrap();
        assert!(ok.success);
        assert_eq!(ok.stdout.trim(), "out");
        assert_eq!(ok.stderr.trim(), "err");

        let failed = run(&Invocation::new("/bin/sh").args(["-c", "exit 3"]), None)
            .await
            .unwrap();
        assert!(!failed.success);
    }

    #[test]
    fn the_stderr_tail_stays_short() {
        let long = Completed {
            success: false,
            stdout: String::new(),
            stderr: "x".repeat(2000),
        };
        assert!(long.stderr_tail().chars().count() <= 402);

        let short = Completed {
            success: false,
            stdout: String::new(),
            stderr: "  ERROR: nope  ".to_owned(),
        };
        assert_eq!(short.stderr_tail(), "ERROR: nope");
    }
}
