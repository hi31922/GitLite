# GitLite

GitLite is a native Windows interface for an installed copy of Git for Windows.

- **Open** a working repository or **Clone** into an empty folder.
- To upload a folder to an empty GitHub repository, type its local path or use **Browse**, paste the repository’s HTTPS link, and click **Publish folder**. GitLite prepares the local repository, commits the files Git can track, and pushes the branch. It asks for a commit author if Git needs one. For a newly published folder, use **Stage all**, **Commit staged**, and **Push** for later changes.
- Review working-copy and staged diffs. Stage an individual file or all changes, then write a commit message and commit the index.
- Switch or create a branch, review recent commits, and fetch, pull, or push when the repository is connected to a remote.
- Use **Author** to set the commit name and email for the open repository.
- **Connect GitHub** opens browser authentication with Git Credential Manager. The Windows credential store holds the sign-in. Git continues to use the repository's configured remotes and credentials.
- Press **F5** to refresh, **Ctrl+O** to choose a repository, or **Ctrl+Enter** to commit staged changes.

If Git is in a custom location, choose **Locate Git**. The program remembers the executable path in the current user's LocalAppData folder.

Build with a Windows Rust toolchain:

~~~powershell
cargo build --release --locked
~~~

The output is **target\release\GitLite.exe**. On the Windows GNU target, use a MinGW-w64 toolchain with dlltool and as available.
