//! ワークスペースの member と、その下のソースを読む。

use std::path::{Path, PathBuf};

/// ワークスペースの member 1つ。
pub struct Member {
    /// `Cargo.toml` の `package.name`。ディレクトリ名ではない。
    pub name: String,
    /// ワークスペースのルートからの相対パス。置き場所の包含（`app/` の下に在るか、など）を
    /// `Path::starts_with` で判定できる。
    pub dir: PathBuf,
    pub manifest: toml::Table,
}

/// ワークスペースのルート。この crate の1つ上。
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("この crate はワークスペースの直下に在る")
        .to_owned()
}

pub fn manifest_at(dir: &Path) -> toml::Table {
    let path = dir.join("Cargo.toml");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .parse()
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `workspace.members` の1項目が指すディレクトリ。
///
/// cargo はグロブを受けるが、escrow が使うのは `app/crates/slices/*` のように**末尾が `*`**
/// の形だけなので、それだけを展開する。他の形はそのまま返し、`Cargo.toml` を読む所で落ちる。
fn expand(root: &Path, member: &str) -> Vec<PathBuf> {
    let Some(group) = member.strip_suffix("/*") else {
        return vec![PathBuf::from(member)];
    };
    let group_dir = root.join(group);
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&group_dir)
        .unwrap_or_else(|e| panic!("{}: {e}", group_dir.display()))
        .map(|entry| entry.expect("読めるエントリ").path())
        .filter(|path| path.is_dir())
        .map(|path| Path::new(group).join(path.file_name().expect("ディレクトリ名")))
        .collect();
    dirs.sort();
    dirs
}

/// `workspace.members` に挙がっているもの全部。
pub fn members() -> Vec<Member> {
    let root = root();
    let mut members: Vec<Member> = manifest_at(&root)["workspace"]["members"]
        .as_array()
        .expect("workspace.members は配列")
        .iter()
        .flat_map(|member| expand(&root, member.as_str().expect("member は文字列")))
        .map(|dir| {
            let manifest = manifest_at(&root.join(&dir));
            let name = manifest["package"]["name"]
                .as_str()
                .expect("package.name は文字列")
                .to_owned();
            Member {
                name,
                dir,
                manifest,
            }
        })
        .collect();
    members.sort_by(|a, b| a.name.cmp(&b.name));
    members
}

impl Member {
    /// この member の `.rs` を、member 相対のパスと中身で返す。
    pub fn sources(&self) -> Vec<(String, String)> {
        fn walk(dir: &Path, root: &Path, found: &mut Vec<(String, String)>) {
            let entries =
                std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
            for entry in entries {
                let path = entry.expect("読めるエントリ").path();
                if path.is_dir() {
                    walk(&path, root, found);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let name = path
                        .strip_prefix(root)
                        .expect("root の下")
                        .to_string_lossy()
                        .into_owned();
                    found.push((name, std::fs::read_to_string(&path).expect("読める")));
                }
            }
        }

        let dir = root().join(&self.dir);
        let mut found = Vec::new();
        walk(&dir, &dir, &mut found);
        found.sort();
        found
    }

    /// その member が名前を知っている crate 全部。
    ///
    /// **dev-dependencies も含める。** テストの中でだけ迂回できるなら、迂回路は在る。
    pub fn dependencies(&self) -> std::collections::BTreeSet<String> {
        ["dependencies", "dev-dependencies", "build-dependencies"]
            .iter()
            .filter_map(|table| self.manifest.get(*table))
            .filter_map(toml::Value::as_table)
            .flat_map(toml::Table::keys)
            .cloned()
            .collect()
    }

    /// その member が名前を知っている escrow-* の crate。
    pub fn escrow_dependencies(&self) -> std::collections::BTreeSet<String> {
        self.dependencies()
            .into_iter()
            .filter(|name| name.starts_with("escrow-"))
            .collect()
    }

    /// `[lib] crate-type`。無ければ空（cargo の既定は `lib` だけ）。
    pub fn crate_types(&self) -> Vec<String> {
        self.manifest
            .get("lib")
            .and_then(|lib| lib.get("crate-type"))
            .and_then(toml::Value::as_array)
            .map(|types| {
                types
                    .iter()
                    .filter_map(toml::Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }
}
