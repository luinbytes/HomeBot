//! Safe executable discovery shared by first-class local providers.

use std::{ffi::OsString, path::PathBuf};

/// Returns the inherited search path plus conventional package-manager locations on macOS.
///
/// Applications opened by Finder receive a deliberately small `PATH`; Codex and Claude are
/// commonly installed by Homebrew or npm beneath `/opt/homebrew` or `/usr/local`. Adding only
/// these fixed locations preserves the provider environment allow-list without evaluating a
/// shell or a login profile.
pub(crate) fn executable_search_path(inherited: Option<&OsString>) -> Option<OsString> {
    let mut directories = inherited
        .map(std::env::split_paths)
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

    if cfg!(target_os = "macos") {
        append_unique(&mut directories, PathBuf::from("/opt/homebrew/bin"));
        append_unique(&mut directories, PathBuf::from("/usr/local/bin"));
    }

    (!directories.is_empty())
        .then(|| std::env::join_paths(directories).ok())
        .flatten()
}

fn append_unique(directories: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !directories.contains(&candidate) {
        directories.push(candidate);
    }
}

#[cfg(test)]
mod tests {
    use super::append_unique;
    use std::path::PathBuf;

    #[test]
    fn fixed_provider_directories_are_not_duplicated() {
        let mut paths = vec![PathBuf::from("/usr/bin")];
        append_unique(&mut paths, PathBuf::from("/usr/local/bin"));
        append_unique(&mut paths, PathBuf::from("/usr/local/bin"));
        assert_eq!(
            paths,
            vec![PathBuf::from("/usr/bin"), PathBuf::from("/usr/local/bin")]
        );
    }
}
