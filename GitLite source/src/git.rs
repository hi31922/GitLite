use std::collections::BTreeMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const HISTORY_LIMIT: &str = "60";
const PREVIEW_LIMIT: usize = 512 * 1024;
const PUBLISH_PENDING: &str = "gitlite.publishPending";
pub const PUBLISH_IDENTITY_REQUIRED: &str = "Set the Git author for this folder before publishing.";

pub fn github_publish_url(input: &str) -> Result<String, String> {
    let invalid = || "Enter a GitHub HTTPS URL such as https://github.com/OWNER/REPO.".to_string();
    let (scheme, rest) = input.trim().split_once("://").ok_or_else(invalid)?;
    let (host, path) = rest.split_once('/').ok_or_else(invalid)?;
    if !scheme.eq_ignore_ascii_case("https") || !host.eq_ignore_ascii_case("github.com") {
        return Err(invalid());
    }
    let path = path.strip_suffix('/').unwrap_or(path);
    let (owner, repo) = path.split_once('/').ok_or_else(invalid)?;
    let repo = if repo.to_ascii_lowercase().ends_with(".git") {
        &repo[..repo.len() - 4]
    } else {
        repo
    };
    if owner.is_empty()
        || owner.len() > 39
        || !owner.as_bytes()[0].is_ascii_alphanumeric()
        || !owner.as_bytes()[owner.len() - 1].is_ascii_alphanumeric()
        || !owner
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || repo.is_empty()
        || repo.len() > 100
        || matches!(repo, "." | "..")
        || repo.eq_ignore_ascii_case(".git")
        || !repo
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(invalid());
    }
    Ok(format!("https://github.com/{owner}/{repo}.git"))
}

#[derive(Clone, Debug)]
pub struct Change {
    pub path: PathBuf,
    pub original_path: Option<PathBuf>,
    pub index_status: char,
    pub worktree_status: char,
    pub staged: bool,
    pub unstaged: bool,
}

#[derive(Clone, Debug)]
pub struct Branch {
    pub name: String,
    pub current: bool,
    pub upstream: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Remote {
    pub name: String,
    pub url: String,
    pub push_url: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Commit {
    pub id: String,
    pub short_id: String,
    pub author: String,
    pub date: String,
    pub subject: String,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub branch: String,
    pub branches: Vec<Branch>,
    pub remotes: Vec<Remote>,
    pub changes: Vec<Change>,
    pub history: Vec<Commit>,
    pub ahead: usize,
    pub behind: usize,
    pub detached: bool,
    pub unborn: bool,
    pub upstream: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Git {
    executable: PathBuf,
}

pub fn find_git() -> io::Result<PathBuf> {
    let executable = if cfg!(windows) { "git.exe" } else { "git" };
    let mut locations = Vec::new();
    if let Some(path) = env::var_os("PATH") {
        locations.extend(env::split_paths(&path).map(|folder| folder.join(executable)));
    }
    if cfg!(windows) {
        for variable in ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(base) = env::var_os(variable) {
                locations.push(PathBuf::from(base).join("Git").join("cmd").join(executable));
            }
        }
        if let Some(base) = env::var_os("LOCALAPPDATA") {
            locations.push(
                PathBuf::from(base)
                    .join("Programs")
                    .join("Git")
                    .join("cmd")
                    .join(executable),
            );
        }
    }
    locations
        .into_iter()
        .find(|path| path.is_absolute() && path.is_file() && validate_git(path).is_ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "Git was not found. Choose git.exe from your Git installation.",
            )
        })
}

pub fn validate_git(executable: &Path) -> Result<(), String> {
    if !executable.is_absolute()
        || !executable.is_file()
        || (cfg!(windows)
            && !executable
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|extension| extension.eq_ignore_ascii_case("exe")))
    {
        return Err("Choose the full path to the Git executable.".into());
    }
    let output = hidden_command(executable)
        .arg("--version")
        .output()
        .map_err(|error| format!("Could not start Git: {error}"))?;
    if output.status.success() && output.stdout.starts_with(b"git version ") {
        Ok(())
    } else {
        Err("The selected program did not identify itself as Git.".into())
    }
}

impl Git {
    pub fn new(executable: PathBuf) -> Self {
        Self { executable }
    }

    pub fn open(&self, folder: &Path) -> Result<PathBuf, String> {
        let output = self.output(Some(folder), ["rev-parse", "--show-toplevel"])?;
        if output.status.success() {
            return git_path(line_bytes(&output.stdout));
        }
        let bare = self.output(Some(folder), ["rev-parse", "--is-bare-repository"])?;
        if bare.status.success() && line_bytes(&bare.stdout) == b"true" {
            return Err(
                "This is a bare repository. Choose a repository with a working tree.".into(),
            );
        }
        Err(command_error(&output, "Open repository"))
    }

    pub fn snapshot(&self, repo: &Path) -> Result<Snapshot, String> {
        let mut snapshot = self.worktree_status(repo)?;
        snapshot.branches = self.branches(repo)?;
        snapshot.remotes = self.remotes(repo)?;
        if !snapshot.unborn {
            let history = required(
                self.output(
                    Some(repo),
                    [
                        "log",
                        "--max-count",
                        HISTORY_LIMIT,
                        "--no-show-signature",
                        "--format=%H%x00%h%x00%an%x00%aI%x00%s",
                        "-z",
                    ],
                )?,
                "Read commit history",
            )?;
            snapshot.history = parse_history(&history.stdout)?;
        }
        Ok(snapshot)
    }

    fn worktree_status(&self, repo: &Path) -> Result<Snapshot, String> {
        let status = required(
            self.output(
                Some(repo),
                [
                    "--no-optional-locks",
                    "status",
                    "--porcelain=v2",
                    "--branch",
                    "--ahead-behind",
                    "--untracked-files=all",
                    "-z",
                ],
            )?,
            "Read repository status",
        )?;
        parse_status(&status.stdout)
    }

    /// Resolves the exact selected repository folder, for author setup after the first publish attempt.
    pub fn publish_root(&self, folder: &Path) -> Result<PathBuf, String> {
        let (path, initialized) = self.inspect_publish_folder(folder)?;
        if initialized {
            Ok(path)
        } else {
            Err("Start Publish before setting the Git author for this folder.".into())
        }
    }

    pub fn publish_folder(&self, folder: &Path, url: &str) -> Result<String, String> {
        let url = github_publish_url(url)?;
        let (folder, initialized) = self.inspect_publish_folder(folder)?;
        let existing = if initialized {
            self.reject_in_progress(&folder)?;
            Some(self.worktree_status(&folder)?)
        } else {
            None
        };
        if existing.as_ref().is_some_and(|status| status.detached) {
            return Err("Switch to a local branch before publishing this folder.".into());
        }
        let branch = existing
            .as_ref()
            .map_or_else(|| "main".to_owned(), |status| status.branch.clone());
        let goal = format!("refs/heads/{branch}");
        let pending = if initialized {
            self.config_value(&folder, PUBLISH_PENDING, true)?
        } else {
            None
        };
        let already_prepared = pending
            .as_deref()
            .and_then(|pending| pending_commit(pending, &url, &goal));
        let effective = required(
            self.output(Some(&folder), ["ls-remote", "--get-url", "--", &url])?,
            "Resolve the GitHub repository",
        )?;
        let effective = git_text(line_bytes(&effective.stdout))?;
        let remote = required(
            self.output(Some(&folder), ["ls-remote", "--refs", "--", &url])?,
            "Check the GitHub repository",
        )?;
        if !remote.stdout.is_empty() {
            if let Some(commit) = already_prepared {
                if self.publish_head(&folder)? == commit
                    && remote_has(&remote.stdout, &goal, commit)
                {
                    let origin = self.origin_matches(&folder, &effective)?;
                    let preserve = self.preserves_push_target(&folder, &branch, &goal, origin)?;
                    return Ok(self.published_result(&folder, &branch, preserve));
                }
            }
            return Err(
                "This GitHub repository already has content. Publish requires an empty repository."
                    .into(),
            );
        }

        if !initialized {
            self.action(
                &folder,
                ["init", "--initial-branch=main"],
                "Initialize Git repository",
            )?;
        }
        let folder = self.publish_root(&folder)?;
        let status = self.worktree_status(&folder)?;
        if status.detached || status.branch != branch {
            return Err(
                "The current branch changed while checking the remote. Publish again.".into(),
            );
        }
        let reuse_commit = if let Some(commit) = already_prepared {
            !status.unborn && self.publish_head(&folder)? == commit
        } else {
            false
        };
        if !reuse_commit {
            self.prepare_publish_commit(&folder, &status)?;
        }
        let commit = self.publish_head(&folder)?;
        let state = format!("{url}\t{goal}\t{commit}");
        self.action(
            &folder,
            [
                "config",
                "--local",
                "--replace-all",
                "--",
                PUBLISH_PENDING,
                &state,
            ],
            "Record the prepared publication",
        )?;
        let destination = self.publish_destination(&folder, &url, &effective)?;
        let preserve =
            self.preserves_push_target(&folder, &branch, &goal, destination == "origin")?;
        let track = destination == "origin"
            && self
                .config_value(&folder, &format!("branch.{branch}.remote"), false)?
                .is_none()
            && self
                .config_value(&folder, &format!("branch.{branch}.merge"), false)?
                .is_none();
        if self.current_ref(&folder, "publishing")? != goal || self.publish_head(&folder)? != commit
        {
            return Err("The branch changed before the push. Publish again.".into());
        }

        let reference = format!("{commit}:{goal}");
        // An empty expected value asserts that this remote ref must not exist at push time.
        let create_only = format!("--force-with-lease={goal}:");
        let mut push = self.command(Some(&folder));
        push.args([
            "push",
            "--no-force",
            "--no-mirror",
            "--no-follow-tags",
            "--recurse-submodules=no",
            &create_only,
        ]);
        push.args(["--", &destination, &reference]);
        action_result(push.output().map_err(start_error)?, "Publish to GitHub")?;
        if track
            && self.current_ref(&folder, "publishing").ok().as_deref() == Some(&goal)
            && self
                .config_value(&folder, &format!("branch.{branch}.remote"), false)
                .ok()
                == Some(None)
            && self
                .config_value(&folder, &format!("branch.{branch}.merge"), false)
                .ok()
                == Some(None)
        {
            let upstream = format!("origin/{branch}");
            let _ = self.output(
                Some(&folder),
                ["branch", "--set-upstream-to", &upstream, "--", &branch],
            );
        }
        Ok(self.published_result(&folder, &branch, preserve))
    }

    fn inspect_publish_folder(&self, folder: &Path) -> Result<(PathBuf, bool), String> {
        let canonical = fs::canonicalize(folder)
            .map_err(|error| format!("Could not access the selected folder: {error}"))?;
        if !canonical.is_dir() {
            return Err("Choose a folder to publish.".into());
        }
        let output = self
            .command(Some(folder))
            .env("LC_ALL", "C")
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .map_err(start_error)?;
        if output.status.success() {
            let root = git_path(line_bytes(&output.stdout))?;
            if fs::canonicalize(root).ok().as_ref() != Some(&canonical) {
                return Err("Choose the root of the Git repository, not a subfolder.".into());
            }
            return Ok((folder.to_path_buf(), true));
        }
        if !String::from_utf8_lossy(&output.stderr).contains("not a git repository")
            || canonical
                .ancestors()
                .any(|parent| match fs::symlink_metadata(parent.join(".git")) {
                    Ok(_) => true,
                    Err(error) => error.kind() != io::ErrorKind::NotFound,
                })
        {
            return Err(command_error(&output, "Check the selected folder"));
        }
        Ok((folder.to_path_buf(), false))
    }

    fn prepare_publish_commit(&self, repo: &Path, status: &Snapshot) -> Result<(), String> {
        let staged = status.changes.iter().any(|change| change.staged);
        if !staged && status.changes.is_empty() {
            return if status.unborn {
                Err("There are no eligible files to commit in this folder.".into())
            } else {
                Ok(())
            };
        }
        for role in ["GIT_AUTHOR_IDENT", "GIT_COMMITTER_IDENT"] {
            let result = self.output(Some(repo), ["var", role])?;
            if !result.status.success() {
                return Err(PUBLISH_IDENTITY_REQUIRED.into());
            }
        }
        self.reject_embedded_repos(repo, status)?;
        if staged {
            self.commit(
                repo,
                if status.unborn {
                    "Initial commit"
                } else {
                    "Publish staged folder changes"
                },
            )?;
            let remaining = self.worktree_status(repo)?;
            self.reject_embedded_repos(repo, &remaining)?;
            if remaining.changes.is_empty() {
                return Ok(());
            }
        }
        self.stage_all(repo)?;
        if !self
            .worktree_status(repo)?
            .changes
            .iter()
            .any(|change| change.staged)
        {
            return if status.unborn && !staged {
                Err("There are no eligible files to commit in this folder.".into())
            } else {
                Ok(())
            };
        }
        let message = if staged {
            "Publish remaining folder changes"
        } else if status.unborn {
            "Initial commit"
        } else {
            "Publish folder changes"
        };
        self.commit(repo, message)?;
        Ok(())
    }

    fn reject_embedded_repos(&self, repo: &Path, status: &Snapshot) -> Result<(), String> {
        for change in &status.changes {
            if change.index_status == '?'
                && repo
                    .join(&change.path)
                    .join(".git")
                    .symlink_metadata()
                    .is_ok()
            {
                return Err(format!(
                    "The folder contains an embedded Git repository at {}. Ignore it or publish it separately.",
                    change.path.display()
                ));
            }
        }
        Ok(())
    }

    fn reject_in_progress(&self, repo: &Path) -> Result<(), String> {
        let unmerged = required(
            self.output(Some(repo), ["ls-files", "--unmerged", "-z"])?,
            "Check unresolved changes",
        )?;
        if !unmerged.stdout.is_empty() {
            return Err("Resolve this repository’s merge conflicts before publishing.".into());
        }
        let markers = required(
            self.output(
                Some(repo),
                [
                    "rev-parse",
                    "--git-path",
                    "MERGE_HEAD",
                    "--git-path",
                    "CHERRY_PICK_HEAD",
                    "--git-path",
                    "REVERT_HEAD",
                    "--git-path",
                    "rebase-merge",
                    "--git-path",
                    "rebase-apply",
                    "--git-path",
                    "sequencer",
                ],
            )?,
            "Check in-progress Git operations",
        )?;
        for path in markers
            .stdout
            .split(|byte| *byte == b'\n')
            .filter(|path| !path.is_empty())
        {
            if repo.join(git_path(line_bytes(path))?).exists() {
                return Err("Finish the in-progress Git operation before publishing.".into());
            }
        }
        Ok(())
    }

    fn publish_destination(
        &self,
        repo: &Path,
        url: &str,
        effective: &str,
    ) -> Result<String, String> {
        let names = required(
            self.output(Some(repo), ["remote"])?,
            "Read configured remotes",
        )?;
        if !names
            .stdout
            .split(|byte| *byte == b'\n')
            .any(|name| line_bytes(name) == b"origin")
        {
            self.action(
                repo,
                ["remote", "add", "--", "origin", url],
                "Add GitHub origin",
            )?;
        }
        if self.origin_matches(repo, effective)? {
            return Ok("origin".into());
        }
        self.check_publish_push_rewrites(repo, url, effective)?;
        Ok(url.into())
    }

    fn check_publish_push_rewrites(
        &self,
        repo: &Path,
        url: &str,
        effective: &str,
    ) -> Result<(), String> {
        let aliases = self.output(
            Some(repo),
            [
                "config",
                "--null",
                "--get-regexp",
                r"^url\..*\.pushinsteadof$",
            ],
        )?;
        if aliases.status.code() == Some(1) && aliases.stderr.is_empty() {
            return Ok(());
        }
        let aliases = required(aliases, "Read Git push URL rewrites")?;
        for record in aliases
            .stdout
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
        {
            let text = git_text(record)?;
            let (key, prefix) = text.split_once('\n').ok_or_else(parse_error)?;
            let replacement = key
                .strip_prefix("url.")
                .and_then(|key| key.strip_suffix(".pushinsteadof"))
                .ok_or_else(parse_error)?;
            if let Some(remainder) = url
                .strip_prefix(prefix)
                .or_else(|| effective.strip_prefix(prefix))
            {
                let rewritten = format!("{replacement}{remainder}");
                if !same_publish_url(rewritten.as_bytes(), effective) {
                    return Err(
                        "Git’s push URL configuration points to a different destination.".into(),
                    );
                }
            }
        }
        Ok(())
    }

    fn origin_matches(&self, repo: &Path, effective: &str) -> Result<bool, String> {
        let fetch = self.output(Some(repo), ["remote", "get-url", "origin"])?;
        let push = self.output(
            Some(repo),
            ["remote", "get-url", "--push", "--all", "origin"],
        )?;
        Ok(fetch.status.success()
            && push.status.success()
            && same_publish_url(line_bytes(&fetch.stdout), effective)
            && same_publish_url(line_bytes(&push.stdout), effective))
    }

    fn preserves_push_target(
        &self,
        repo: &Path,
        branch: &str,
        goal: &str,
        origin: bool,
    ) -> Result<bool, String> {
        if !origin {
            return Ok(true);
        }
        let remote = self.config_value(repo, &format!("branch.{branch}.remote"), false)?;
        let reference = self.config_value(repo, &format!("branch.{branch}.merge"), false)?;
        Ok(match (remote.as_deref(), reference.as_deref()) {
            (None, None) => false,
            (Some("origin"), Some(reference)) if reference == goal => false,
            _ => true,
        })
    }

    fn publish_head(&self, repo: &Path) -> Result<String, String> {
        let output = required(
            self.output(Some(repo), ["rev-parse", "--verify", "HEAD^{commit}"])?,
            "Read the prepared commit",
        )?;
        git_text(line_bytes(&output.stdout))
    }

    fn published_result(&self, repo: &Path, branch: &str, preserve: bool) -> String {
        let remaining = self
            .worktree_status(repo)
            .is_ok_and(|status| !status.changes.is_empty());
        let _ = self.output(
            Some(repo),
            ["config", "--local", "--unset-all", "--", PUBLISH_PENDING],
        );
        let result = if preserve {
            format!("Published {branch}. Existing Push destination preserved.")
        } else {
            format!("Published {branch} to GitHub.")
        };
        if remaining {
            format!("{result} Uncommitted changes remain local.")
        } else {
            result
        }
    }

    pub fn diff(&self, repo: &Path, change: &Change, staged: bool) -> Result<String, String> {
        check_relative(&change.path)?;
        let mut command = self.command(Some(repo));
        command.args(["diff", "--no-ext-diff", "--no-textconv", "--no-color"]);
        if change.index_status == '?' {
            if staged {
                return Ok(String::new());
            }
            command
                .args(["--no-index", "--", "/dev/null"])
                .arg(&change.path);
            let output = command.output().map_err(start_error)?;
            if output.status.success() || output.status.code() == Some(1) {
                return Ok(preview(&output.stdout));
            }
            return Err(command_error(&output, "Read untracked diff"));
        }
        if staged {
            command.arg("--cached");
        }
        command.arg("--");
        add_change_paths(
            &mut command,
            change,
            if staged {
                change.index_status
            } else {
                change.worktree_status
            },
        )?;
        let output = required(command.output().map_err(start_error)?, "Read diff")?;
        Ok(preview(&output.stdout))
    }

    pub fn stage(&self, repo: &Path, change: &Change) -> Result<String, String> {
        let mut command = self.command(Some(repo));
        command.args(["add", "--all", "--"]);
        add_change_paths(&mut command, change, change.worktree_status)?;
        action_result(command.output().map_err(start_error)?, "Stage change")
    }

    pub fn stage_all(&self, repo: &Path) -> Result<String, String> {
        self.action(repo, ["add", "--all", "--", "."], "Stage all changes")
    }

    pub fn unstage(&self, repo: &Path, change: &Change) -> Result<String, String> {
        let head = self.output(Some(repo), ["rev-parse", "--verify", "--quiet", "HEAD"])?;
        let mut command = self.command(Some(repo));
        if head.status.success() {
            command.args(["restore", "--staged", "--source=HEAD", "--"]);
        } else if head.status.code() == Some(1) {
            // An initial commit has no HEAD to restore. Removing only the index entries preserves the files.
            command.args(["update-index", "--force-remove", "--"]);
        } else {
            return Err(command_error(&head, "Read HEAD"));
        }
        add_change_paths(&mut command, change, change.index_status)?;
        action_result(command.output().map_err(start_error)?, "Unstage change")
    }

    pub fn commit(&self, repo: &Path, message: &str) -> Result<String, String> {
        if message.trim().is_empty() {
            return Err("Enter a commit message.".into());
        }
        self.action(repo, ["commit", "--message", message], "Commit")
    }

    pub fn commit_detail(&self, repo: &Path, id: &str) -> Result<String, String> {
        if !(4..=64).contains(&id.len()) || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("Choose a commit from the history.".into());
        }
        let output = required(
            self.output(
                Some(repo),
                [
                    "show",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-color",
                    "--no-show-signature",
                    "--format=fuller",
                    "--stat",
                    "--patch",
                    id,
                    "--",
                ],
            )?,
            "Read commit details",
        )?;
        Ok(preview(&output.stdout))
    }

    pub fn switch(&self, repo: &Path, branch: &str) -> Result<String, String> {
        self.check_branch(repo, branch)?;
        self.action(
            repo,
            [
                "switch",
                "--no-guess",
                "--no-overwrite-ignore",
                "--",
                branch,
            ],
            "Switch branch",
        )
    }

    pub fn create_branch(&self, repo: &Path, branch: &str) -> Result<String, String> {
        self.check_branch(repo, branch)?;
        self.action(
            repo,
            [
                "switch",
                "--no-guess",
                "--no-overwrite-ignore",
                "--create",
                branch,
            ],
            "Create branch",
        )
    }

    pub fn clone_repo(&self, url: &str, destination: &Path) -> Result<String, String> {
        if url.trim().is_empty() || !destination.is_absolute() {
            return Err("Enter a repository address and choose a full destination folder.".into());
        }
        let output = self
            .command(None)
            .args(["clone", "--no-recurse-submodules", "--", url.trim()])
            .arg(destination)
            .output()
            .map_err(start_error)?;
        action_result(output, "Clone repository")
    }

    pub fn fetch(&self, repo: &Path) -> Result<String, String> {
        self.action(
            repo,
            [
                "fetch",
                "--no-recurse-submodules",
                "--no-prune",
                "--no-prune-tags",
            ],
            "Fetch",
        )
    }

    pub fn pull(&self, repo: &Path) -> Result<String, String> {
        let local = self.current_ref(repo, "pulling")?;
        let branch = local.strip_prefix("refs/heads/").ok_or_else(parse_error)?;
        let remote_key = format!("branch.{branch}.remote");
        let upstream_key = format!("branch.{branch}.merge");
        let remote = self.config_value(repo, &remote_key, false)?;
        let upstream = self.config_value(repo, &upstream_key, false)?;
        let (Some(remote), Some(upstream)) = (remote, upstream) else {
            return Err("Set an upstream branch before pulling.".into());
        };
        let fetched = self.action(
            repo,
            [
                "fetch",
                "--no-recurse-submodules",
                "--no-prune",
                "--no-prune-tags",
                "--",
                &remote,
            ],
            "Fetch upstream",
        )?;

        if self.current_ref(repo, "pulling")? != local
            || self.config_value(repo, &remote_key, false)?.as_deref() != Some(&remote)
            || self.config_value(repo, &upstream_key, false)?.as_deref() != Some(&upstream)
        {
            return Err(
                "The current branch or its upstream changed during fetch. Pull again.".into(),
            );
        }
        let upstream_commit = required(
            self.output(
                Some(repo),
                [
                    "rev-parse",
                    "--verify",
                    "--end-of-options",
                    "@{upstream}^{commit}",
                ],
            )?,
            "Find fetched upstream commit",
        )?;
        let upstream_commit = git_text(line_bytes(&upstream_commit.stdout))?;
        let merged = self.action(
            repo,
            [
                "merge",
                "--ff-only",
                "--no-autostash",
                "--no-overwrite-ignore",
                "--",
                &upstream_commit,
            ],
            "Pull",
        )?;
        Ok([fetched, merged]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n"))
    }

    pub fn push(&self, repo: &Path) -> Result<String, String> {
        let local = self.current_ref(repo, "pushing")?;
        let branch = local.strip_prefix("refs/heads/").ok_or_else(parse_error)?;
        let upstream_remote = self.config_value(repo, &format!("branch.{branch}.remote"), false)?;
        let upstream_ref = self.config_value(repo, &format!("branch.{branch}.merge"), false)?;
        let remotes = self.remotes(repo)?;
        let (remote, target, set_upstream) = match (upstream_remote, upstream_ref) {
            (Some(remote), Some(target)) => (remote, target, false),
            (None, None) => {
                let remote = remotes
                    .iter()
                    .find(|remote| remote.name == "origin")
                    .or_else(|| {
                        if remotes.len() == 1 {
                            remotes.first()
                        } else {
                            None
                        }
                    })
                    .ok_or_else(|| "Set an upstream for this branch before pushing.".to_string())?;
                (remote.name.clone(), local.clone(), true)
            }
            _ => return Err("This branch has an incomplete upstream configuration.".into()),
        };
        if !target.starts_with("refs/heads/") || !remotes.iter().any(|item| item.name == remote) {
            return Err("This branch has no usable remote branch to push to.".into());
        }
        required(
            self.output(Some(repo), ["check-ref-format", &target])?,
            "Validate upstream branch",
        )?;
        let specification = format!("{local}:{target}");
        let mut command = self.command(Some(repo));
        command.args([
            "push",
            "--no-force",
            "--no-mirror",
            "--no-follow-tags",
            "--recurse-submodules=no",
        ]);
        if set_upstream {
            command.arg("--set-upstream");
        }
        command.args(["--", &remote, &specification]);
        action_result(command.output().map_err(start_error)?, "Push")
    }

    pub fn set_identity(&self, repo: &Path, name: &str, email: &str) -> Result<String, String> {
        let name = name.trim();
        let email = email.trim();
        if name.is_empty()
            || email.is_empty()
            || name.chars().chain(email.chars()).any(char::is_control)
        {
            return Err("Enter a Git author name and email.".into());
        }
        let old_name = self.config_value(repo, "user.name", true)?;
        self.action(
            repo,
            [
                "config",
                "--local",
                "--replace-all",
                "--",
                "user.name",
                name,
            ],
            "Save Git author name",
        )?;
        let email_result = self.action(
            repo,
            [
                "config",
                "--local",
                "--replace-all",
                "--",
                "user.email",
                email,
            ],
            "Save Git author email",
        );
        if let Err(error) = email_result {
            let restored = match old_name {
                Some(old) => self.action(
                    repo,
                    [
                        "config",
                        "--local",
                        "--replace-all",
                        "--",
                        "user.name",
                        &old,
                    ],
                    "Restore Git author name",
                ),
                None => self.action(
                    repo,
                    ["config", "--local", "--unset-all", "--", "user.name"],
                    "Restore Git author name",
                ),
            };
            return match restored {
                Ok(_) => Err(error),
                Err(restore) => Err(format!("{error}\n{restore}")),
            };
        }
        Ok("Saved the Git author for this repository.".into())
    }

    fn check_branch(&self, repo: &Path, branch: &str) -> Result<(), String> {
        if branch.is_empty() || branch.starts_with('-') || branch.starts_with("@{") {
            return Err("Enter a valid local branch name.".into());
        }
        required(
            self.output(Some(repo), ["check-ref-format", "--branch", branch])?,
            "Validate branch name",
        )?;
        Ok(())
    }

    fn current_ref(&self, repo: &Path, operation: &str) -> Result<String, String> {
        let head = self.output(Some(repo), ["symbolic-ref", "--quiet", "HEAD"])?;
        if head.status.code() == Some(1) {
            return Err(format!("Switch to a local branch before {operation}."));
        }
        git_text(line_bytes(&required(head, "Read current branch")?.stdout))
    }

    fn branches(&self, repo: &Path) -> Result<Vec<Branch>, String> {
        let output = required(
            self.output(
                Some(repo),
                [
                    "for-each-ref",
                    "--sort=refname",
                    "--format=%(refname:lstrip=2)%00%(HEAD)%00%(upstream:short)",
                    "refs/heads/",
                ],
            )?,
            "Read branches",
        )?;
        let mut result = Vec::new();
        for line in output
            .stdout
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let mut fields = line_bytes(line).split(|byte| *byte == 0);
            let name = git_text(fields.next().ok_or_else(parse_error)?)?;
            let current = fields.next().ok_or_else(parse_error)? == b"*";
            let tracking = fields.next().ok_or_else(parse_error)?;
            let upstream = if tracking.is_empty() {
                None
            } else {
                Some(git_text(tracking)?)
            };
            result.push(Branch {
                name,
                current,
                upstream,
            });
        }
        Ok(result)
    }

    fn remotes(&self, repo: &Path) -> Result<Vec<Remote>, String> {
        let output = self.output(
            Some(repo),
            [
                "config",
                "--null",
                "--get-regexp",
                r"^remote\..*\.(url|pushurl)$",
            ],
        )?;
        if output.status.code() == Some(1) && output.stdout.is_empty() && output.stderr.is_empty() {
            return Ok(Vec::new());
        }
        let output = required(output, "Read remotes")?;
        let mut remotes = BTreeMap::new();
        for record in output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
        {
            let mut fields = record.splitn(2, |byte| *byte == b'\n');
            let key = git_text(fields.next().ok_or_else(parse_error)?)?;
            let url = sanitized(fields.next().ok_or_else(parse_error)?);
            let key = key.strip_prefix("remote.").ok_or_else(parse_error)?;
            let (name, field) = key.rsplit_once('.').ok_or_else(parse_error)?;
            let remote = remotes.entry(name.to_owned()).or_insert_with(|| Remote {
                name: name.to_owned(),
                url: String::new(),
                push_url: None,
            });
            if field == "url" && remote.url.is_empty() {
                remote.url = url;
            } else if field == "pushurl" && remote.push_url.is_none() {
                remote.push_url = Some(url);
            }
        }
        Ok(remotes.into_values().collect())
    }

    fn config_value(&self, repo: &Path, name: &str, local: bool) -> Result<Option<String>, String> {
        let mut command = self.command(Some(repo));
        command.arg("config");
        if local {
            command.arg("--local");
        }
        let output = command
            .args(["--null", "--get", "--", name])
            .output()
            .map_err(start_error)?;
        if output.status.code() == Some(1) && output.stderr.is_empty() {
            return Ok(None);
        }
        let output = required(output, "Read Git configuration")?;
        let value = output.stdout.strip_suffix(&[0]).ok_or_else(parse_error)?;
        Ok(Some(git_text(value)?))
    }

    fn action<const N: usize>(
        &self,
        repo: &Path,
        args: [&str; N],
        name: &str,
    ) -> Result<String, String> {
        action_result(self.output(Some(repo), args)?, name)
    }

    fn output<I, S>(&self, repo: Option<&Path>, args: I) -> Result<Output, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.command(repo).args(args).output().map_err(start_error)
    }

    fn command(&self, repo: Option<&Path>) -> Command {
        let mut command = hidden_command(&self.executable);
        command.args([
            "--no-pager",
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "submodule.recurse=false",
            "-c",
            "protocol.ext.allow=never",
        ]);
        command.env("GIT_TERMINAL_PROMPT", "0");
        command.env("GIT_TRACE_REDACT", "1");
        for key in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_COMMON_DIR",
            "GIT_INDEX_FILE",
            "GIT_PREFIX",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_CONFIG",
            "GIT_CONFIG_PARAMETERS",
            "GIT_CONFIG_COUNT",
            "GIT_GLOB_PATHSPECS",
            "GIT_NOGLOB_PATHSPECS",
            "GIT_ICASE_PATHSPECS",
            "GIT_TRACE",
            "GIT_TRACE_PACKET",
            "GIT_TRACE_CURL",
            "GIT_CURL_VERBOSE",
            "GIT_TRACE2",
            "GIT_TRACE2_EVENT",
            "GIT_TRACE2_PERF",
            "GCM_TRACE",
            "GCM_TRACE_SECRETS",
        ] {
            command.env_remove(key);
        }
        if let Some(repo) = repo {
            command.arg("-C").arg(repo);
        }
        command
    }
}

fn parse_status(bytes: &[u8]) -> Result<Snapshot, String> {
    let mut snapshot = Snapshot {
        branch: String::new(),
        branches: Vec::new(),
        remotes: Vec::new(),
        changes: Vec::new(),
        history: Vec::new(),
        ahead: 0,
        behind: 0,
        detached: false,
        unborn: false,
        upstream: None,
    };
    let mut records = bytes.split(|byte| *byte == 0);
    while let Some(record) = records.next() {
        if let Some(head) = record.strip_prefix(b"# branch.head ") {
            snapshot.detached = head == b"(detached)";
            snapshot.branch = if snapshot.detached {
                "Detached HEAD".into()
            } else {
                git_text(head)?
            };
        } else if let Some(oid) = record.strip_prefix(b"# branch.oid ") {
            snapshot.unborn = oid == b"(initial)";
        } else if let Some(upstream) = record.strip_prefix(b"# branch.upstream ") {
            snapshot.upstream = Some(git_text(upstream)?);
        } else if let Some(distance) = record.strip_prefix(b"# branch.ab ") {
            let text = git_text(distance)?;
            let (ahead, behind) = text.split_once(' ').ok_or_else(parse_error)?;
            snapshot.ahead = ahead
                .strip_prefix('+')
                .ok_or_else(parse_error)?
                .parse()
                .map_err(|_| parse_error())?;
            snapshot.behind = behind
                .strip_prefix('-')
                .ok_or_else(parse_error)?
                .parse()
                .map_err(|_| parse_error())?;
        } else if let Some(path) = record.strip_prefix(b"? ") {
            snapshot.changes.push(Change {
                path: git_path(path)?,
                original_path: None,
                index_status: '?',
                worktree_status: '?',
                staged: false,
                unstaged: true,
            });
        } else if matches!(record.first(), Some(b'1' | b'2' | b'u')) {
            let (field_count, renamed) = match record[0] {
                b'1' => (9, false),
                b'2' => (10, true),
                _ => (11, false),
            };
            let status = record.get(2..4).ok_or_else(parse_error)?;
            let path = record
                .splitn(field_count, |byte| *byte == b' ')
                .nth(field_count - 1)
                .ok_or_else(parse_error)?;
            let original_path = if renamed {
                Some(git_path(records.next().ok_or_else(parse_error)?)?)
            } else {
                None
            };
            snapshot.changes.push(Change {
                path: git_path(path)?,
                original_path,
                index_status: status[0] as char,
                worktree_status: status[1] as char,
                staged: status[0] != b'.',
                unstaged: status[1] != b'.',
            });
        }
    }
    if snapshot.branch.is_empty() {
        return Err(parse_error());
    }
    Ok(snapshot)
}

fn pending_commit<'a>(record: &'a str, url: &str, goal: &str) -> Option<&'a str> {
    let mut fields = record.split('\t');
    let previous_url = fields.next()?;
    let previous_ref = fields.next()?;
    let commit = fields.next()?;
    if previous_url.eq_ignore_ascii_case(url)
        && previous_ref == goal
        && fields.next().is_none()
        && matches!(commit.len(), 40 | 64)
        && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        Some(commit)
    } else {
        None
    }
}

fn remote_has(bytes: &[u8], reference: &str, commit: &str) -> bool {
    bytes.split(|byte| *byte == b'\n').any(|line| {
        let mut fields = line_bytes(line).splitn(2, |byte| *byte == b'\t');
        fields.next() == Some(commit.as_bytes()) && fields.next() == Some(reference.as_bytes())
    })
}

fn same_publish_url(bytes: &[u8], expected: &str) -> bool {
    git_text(bytes).is_ok_and(|url| {
        url == expected
            || matches!((github_publish_url(&url), github_publish_url(expected)), (Ok(left), Ok(right)) if left.eq_ignore_ascii_case(&right))
    })
}

fn parse_history(bytes: &[u8]) -> Result<Vec<Commit>, String> {
    let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let fields: Vec<_> = bytes.split(|byte| *byte == 0).collect();
    if fields.len() % 5 != 0 {
        return Err(parse_error());
    }
    Ok(fields
        .chunks_exact(5)
        .map(|commit| Commit {
            id: String::from_utf8_lossy(commit[0]).into_owned(),
            short_id: String::from_utf8_lossy(commit[1]).into_owned(),
            author: String::from_utf8_lossy(commit[2]).into_owned(),
            date: String::from_utf8_lossy(commit[3]).into_owned(),
            subject: String::from_utf8_lossy(commit[4]).into_owned(),
        })
        .collect())
}

fn check_relative(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("The selected path is outside the repository.".into());
    }
    Ok(())
}

fn add_change_paths(command: &mut Command, change: &Change, status: char) -> Result<(), String> {
    check_relative(&change.path)?;
    command.arg(&change.path);
    if status == 'R' {
        if let Some(original) = &change.original_path {
            check_relative(original)?;
            command.arg(original);
        }
    }
    Ok(())
}

fn git_path(bytes: &[u8]) -> Result<PathBuf, String> {
    if bytes.is_empty() {
        return Err(parse_error());
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(PathBuf::from(OsString::from_vec(bytes.to_vec())))
    }
    #[cfg(not(unix))]
    {
        Ok(PathBuf::from(OsString::from(git_text(bytes)?)))
    }
}

fn git_text(bytes: &[u8]) -> Result<String, String> {
    String::from_utf8(bytes.to_vec())
        .map_err(|_| "Git returned a name that cannot be read as UTF-8.".to_string())
}

fn line_bytes(bytes: &[u8]) -> &[u8] {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    bytes.strip_suffix(b"\r").unwrap_or(bytes)
}

fn hidden_command(executable: &Path) -> Command {
    let mut command = Command::new(executable);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    command
}

fn required(output: Output, action: &str) -> Result<Output, String> {
    if output.status.success() {
        Ok(output)
    } else {
        Err(command_error(&output, action))
    }
}

fn action_result(output: Output, action: &str) -> Result<String, String> {
    let output = required(output, action)?;
    Ok(combined_output(&output))
}

fn combined_output(output: &Output) -> String {
    let stdout = sanitized(&output.stdout);
    let stderr = sanitized(&output.stderr);
    [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn command_error(output: &Output, action: &str) -> String {
    let detail = combined_output(output);
    if detail.is_empty() {
        format!("{action} failed ({})", output.status)
    } else {
        format!("{action} failed.\n{detail}")
    }
}

fn start_error(error: io::Error) -> String {
    format!("Could not start Git: {error}")
}

fn parse_error() -> String {
    "Git returned an unexpected result.".into()
}

fn preview(bytes: &[u8]) -> String {
    if bytes.len() <= PREVIEW_LIMIT {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut text = String::from_utf8_lossy(&bytes[..PREVIEW_LIMIT]).into_owned();
    text.push_str("\n\nPreview truncated.");
    text
}

fn sanitized(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut safe = String::new();
    for token in text.split_inclusive(char::is_whitespace) {
        if let Some(scheme) = token.find("://") {
            let body = &token[scheme + 3..];
            let authority_end = body
                .find(|character: char| {
                    matches!(character, '/' | '?' | '#' | '\'' | '"') || character.is_whitespace()
                })
                .unwrap_or(body.len());
            let authority = &body[..authority_end];
            safe.push_str(&token[..scheme + 3]);
            safe.push_str(
                authority
                    .rsplit_once('@')
                    .map_or(authority, |(_, host)| host),
            );
            let suffix = &body[authority_end..];
            if let Some(query) = suffix.find(['?', '#']) {
                safe.push_str(&suffix[..query]);
                safe.push_str("[redacted]");
                safe.push_str(&suffix[suffix.trim_end().len()..]);
            } else {
                safe.push_str(suffix);
            }
        } else {
            safe.push_str(token);
        }
    }
    safe
}
