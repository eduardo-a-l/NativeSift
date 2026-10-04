use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Open(PathBuf),
    RunCommand {
        command: String,
        working_dir: Option<PathBuf>,
    },
}

#[cfg(target_os = "linux")]
fn opener(target: &PathBuf) -> Command {
    let mut command = Command::new("xdg-open");
    command.arg(target);
    command
}

#[cfg(target_os = "macos")]
fn opener(target: &PathBuf) -> Command {
    let mut command = Command::new("open");
    command.arg(target);
    command
}

#[cfg(windows)]
fn opener(target: &PathBuf) -> Command {
    let mut command = Command::new("explorer.exe");
    command.arg(target);
    command
}

#[cfg(unix)]
fn shell(script: &str) -> Command {
    let mut command = Command::new("sh");
    command.arg("-c").arg(script);
    command
}

#[cfg(windows)]
fn shell(script: &str) -> Command {
    let mut command = Command::new("cmd");
    command.arg("/C").arg(script);
    command
}

pub fn run_action(action: &Action) -> io::Result<()> {
    let mut command = match action {
        Action::Open(path) => opener(path),
        Action::RunCommand {
            command,
            working_dir,
        } => {
            let mut shell_command = shell(command);
            if let Some(directory) = working_dir {
                shell_command.current_dir(directory);
            }
            shell_command
        }
    };
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_shell_commands_without_blocking() {
        let action = Action::RunCommand {
            command: "exit 0".into(),
            working_dir: None,
        };
        assert!(run_action(&action).is_ok());
    }

    #[test]
    fn runs_commands_in_the_requested_directory() {
        let dir = tempfile::tempdir().unwrap();
        let action = Action::RunCommand {
            command: "exit 0".into(),
            working_dir: Some(dir.path().to_path_buf()),
        };
        assert!(run_action(&action).is_ok());
    }

    #[test]
    fn reports_a_missing_working_directory() {
        let action = Action::RunCommand {
            command: "exit 0".into(),
            working_dir: Some(PathBuf::from("/definitely/not/a/directory")),
        };
        assert!(run_action(&action).is_err());
    }
}
