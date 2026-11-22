use std::io::ErrorKind;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const ACCOUNT_LIST_ERROR: &str =
    "Git Credential Manager returned an unfamiliar GitHub account list.";
const MISSING_MANAGER: &str = "Git Credential Manager is unavailable. Install Git for Windows with Git Credential Manager enabled.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountStatus {
    SignedOut,
    SignedIn(String),
    Multiple(Vec<String>),
}

enum Operation {
    List,
    Login,
}

/// Report only the GitHub.com accounts cached by the selected Git installation's helper.
pub fn account(git: &Path) -> Result<AccountStatus, String> {
    parse_accounts(&run(git, Operation::List)?)
}

/// Open the browser for GitHub.com authorization and then refresh the cached account list.
pub fn sign_in(git: &Path) -> Result<AccountStatus, String> {
    run(git, Operation::Login)?;
    match account(git)? {
        AccountStatus::SignedOut => Err(
            "GitHub sign-in finished, but no cached account was found. Try signing in again."
                .into(),
        ),
        status => Ok(status),
    }
}

fn run(git: &Path, operation: Operation) -> Result<Vec<u8>, String> {
    if !git.is_absolute() {
        return Err("Choose a Git for Windows executable before connecting GitHub.".into());
    }

    let mut command = Command::new(git);
    command
        .args(["credential-manager", "github"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_TRACE", "0")
        .env("GCM_TRACE_SECRETS", "0");
    if let Some(directory) = git.parent() {
        command.current_dir(directory);
    }
    match operation {
        Operation::List => {
            command
                .args(["list", "--no-ui"])
                .env("GCM_INTERACTIVE", "0");
        }
        Operation::Login => {
            command.args(["login", "--browser"]).stdout(Stdio::null());
        }
    }

    let output = command.output().map_err(|error| match error.kind() {
        ErrorKind::NotFound => "Git for Windows could not be found at the configured path.",
        ErrorKind::PermissionDenied => "Windows could not run the configured Git executable.",
        _ => "Git for Windows could not be started.",
    })?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(command_error(&operation, &output.stderr).into())
    }
}

fn command_error(operation: &Operation, stderr: &[u8]) -> &'static str {
    // Errors can include authentication URLs or machine-specific data. Report known cases
    // without forwarding Git or Credential Manager output to the UI.
    let error = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    if error.contains("not a git command") || error.contains("cannot spawn git-credential-manager")
    {
        return MISSING_MANAGER;
    }
    if error.contains("unrecognized command")
        || error.contains("unknown command")
        || error.contains("unrecognized option")
    {
        return "This version of Git Credential Manager lacks GitHub account commands. Update Git for Windows.";
    }
    match operation {
        Operation::List => "Could not read the GitHub account list from Git Credential Manager.",
        Operation::Login => {
            if error.contains("interaction has been disabled")
                || error.contains("interaction is disabled")
                || error.contains("interactivity has been disabled")
            {
                "Git Credential Manager’s settings prevent an interactive GitHub sign-in."
            } else if error.contains("browser authentication requires a desktop session") {
                "GitHub sign-in needs a Windows desktop session with a web browser."
            } else if error.contains("cancelled")
                || error.contains("canceled")
                || error.contains("access_denied")
            {
                "GitHub sign-in was canceled or denied."
            } else {
                "GitHub sign-in did not complete. Check your browser and connection, then try again."
            }
        }
    }
}

fn parse_accounts(stdout: &[u8]) -> Result<AccountStatus, String> {
    let text = std::str::from_utf8(stdout).map_err(|_| ACCOUNT_LIST_ERROR)?;
    let mut accounts = Vec::<String>::new();
    for name in text.trim_start_matches('\u{feff}').lines().map(str::trim) {
        if name.is_empty() {
            continue;
        }
        // GitHub.com user names may include an underscore for Enterprise Managed Users.
        let bytes = name.as_bytes();
        if bytes.len() > 39
            || !bytes[0].is_ascii_alphanumeric()
            || !bytes[bytes.len() - 1].is_ascii_alphanumeric()
            || !bytes
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(ACCOUNT_LIST_ERROR.into());
        }
        if !accounts
            .iter()
            .any(|account| account.eq_ignore_ascii_case(name))
        {
            accounts.push(name.into());
        }
    }

    match accounts.len() {
        0 => Ok(AccountStatus::SignedOut),
        1 => Ok(AccountStatus::SignedIn(accounts.remove(0))),
        _ => Ok(AccountStatus::Multiple(accounts)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_empty_and_crlf_account_lists() {
        assert_eq!(parse_accounts(b"\r\n"), Ok(AccountStatus::SignedOut));
        assert_eq!(
            parse_accounts(b"Mona_Example\r\n"),
            Ok(AccountStatus::SignedIn("Mona_Example".into()))
        );
        assert_eq!(
            parse_accounts(b"mona-cat\r\nMONA-CAT\r\nsecond-account\r\n"),
            Ok(AccountStatus::Multiple(vec![
                "mona-cat".into(),
                "second-account".into()
            ]))
        );
    }

    #[test]
    fn unexpected_output_is_not_echoed() {
        assert_eq!(
            parse_accounts(b"fatal: unexpected credential output\n"),
            Err(ACCOUNT_LIST_ERROR.into())
        );
        assert_eq!(parse_accounts(&[0xff]), Err(ACCOUNT_LIST_ERROR.into()));
        assert_eq!(
            command_error(
                &Operation::List,
                b"git: 'credential-manager' is not a git command"
            ),
            MISSING_MANAGER
        );
    }
}
