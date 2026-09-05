//! ワークスペースの形を読む。
//!
//! member の一覧は `Cargo.toml` から取る。ディレクトリの位置を決め打ちにしないので、
//! member をどこへ置いても、下の守りはそこへ届く。

use std::path::{Path, PathBuf};

/// ワークスペースの member 1つ。
pub struct Member {
    /// `Cargo.toml` の `package.name`。ディレクトリ名ではない。
    pub name: String,
    pub dir: PathBuf,
    pub manifest: toml::Table,
}

/// ワークスペースの根。この crate の1つ上。
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

/// `workspace.members` に挙がっているもの全部。
pub fn members() -> Vec<Member> {
    let root = root();
    let mut members: Vec<Member> = manifest_at(&root)["workspace"]["members"]
        .as_array()
        .expect("workspace.members は配列")
        .iter()
        .map(|member| {
            let dir = root.join(member.as_str().expect("member は文字列"));
            let manifest = manifest_at(&dir);
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

        let mut found = Vec::new();
        walk(&self.dir, &self.dir, &mut found);
        found.sort();
        found
    }

    /// その member が名前を知っている escrow-* の crate。
    ///
    /// **dev-dependencies も含める。** テストの中でだけ迂回できるなら、迂回路は在る。
    pub fn escrow_dependencies(&self) -> std::collections::BTreeSet<String> {
        ["dependencies", "dev-dependencies", "build-dependencies"]
            .iter()
            .filter_map(|table| self.manifest.get(*table))
            .filter_map(toml::Value::as_table)
            .flat_map(toml::Table::keys)
            .filter(|name| name.starts_with("escrow-"))
            .cloned()
            .collect()
    }
}
