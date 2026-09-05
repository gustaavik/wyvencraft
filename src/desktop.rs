//! Handing a file to the desktop environment.
//!
//! One job: given a path the game just wrote, show it to the player the way the
//! OS would — reveal it in the file manager, and open it in whatever views that
//! kind of file. Used by the screenshot line in chat.
//!
//! The command *choice* is a pure function so it can be tested on any machine;
//! only [`show_file`] actually spawns anything. Nothing here waits on the child
//! or reads its output: a file manager that takes a second to appear must not
//! stall the frame loop, and a desktop that cannot open the file is worth a
//! warning and nothing more.

use std::path::Path;
use std::process::Command;

/// A program and its arguments, ready to spawn.
type Spawn = (&'static str, Vec<String>);

/// Reveal `path` in the file manager, then open it.
///
/// Both, in that order, so the image ends up in front of the folder it came
/// from. Failures are logged rather than surfaced: this is a convenience on top
/// of a file that has already been written successfully, and the path is in the
/// chat line either way.
pub fn show_file(path: &Path) {
    for (program, args) in [reveal_command(path), open_command(path)] {
        match Command::new(program).args(&args).spawn() {
            // Deliberately not waited on. The child outlives the call by design.
            Ok(_) => {}
            Err(err) => log::warn!("could not run {program} for {}: {err}", path.display()),
        }
    }
}

/// How this platform is asked to reveal a file in its file manager.
///
/// Only macOS and Windows can select the file itself; elsewhere the best
/// available answer is to open the containing directory, which is why this
/// falls back to the parent rather than the file.
pub fn reveal_command(path: &Path) -> Spawn {
    let file = path.display().to_string();
    if cfg!(target_os = "macos") {
        ("open", vec!["-R".to_string(), file])
    } else if cfg!(target_os = "windows") {
        // One argument, comma-joined: `explorer /select,C:\...` is a quirk of
        // explorer's own parsing, not a normal flag-and-value pair.
        ("explorer", vec![format!("/select,{file}")])
    } else {
        let dir = path.parent().unwrap_or(path).display().to_string();
        ("xdg-open", vec![dir])
    }
}

/// How this platform is asked to open a file in its default application.
pub fn open_command(path: &Path) -> Spawn {
    let file = path.display().to_string();
    if cfg!(target_os = "macos") {
        ("open", vec![file])
    } else if cfg!(target_os = "windows") {
        // `start` is a cmd builtin, not an executable, and its first quoted
        // argument is the *window title* — hence the empty one before the path.
        (
            "cmd",
            vec!["/C".to_string(), "start".to_string(), String::new(), file],
        )
    } else {
        ("xdg-open", vec![file])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whatever the platform, both commands must name the file the caller asked
    /// about — a reveal that opens someone else's screenshot would be worse than
    /// one that does nothing.
    #[test]
    fn both_commands_mention_the_file() {
        let path = Path::new("/tmp/shots/2026-09-06_12-00-00.png");
        let name = "2026-09-06_12-00-00.png";

        let (_, open) = open_command(path);
        assert!(
            open.iter().any(|a| a.contains(name)),
            "open command lost the file: {open:?}"
        );

        let (_, reveal) = reveal_command(path);
        assert!(
            reveal
                .iter()
                .any(|a| a.contains(name) || a.contains("/tmp/shots")),
            "reveal command lost the file: {reveal:?}"
        );
    }

    /// The path is passed as its own argument, never interpolated into a shell
    /// string, so a directory with a space or a quote in it cannot become two
    /// arguments or an injection.
    #[test]
    fn a_path_with_spaces_stays_one_argument() {
        let path = Path::new("/tmp/my shots/a b.png");
        let (_, args) = open_command(path);
        assert!(
            args.iter().any(|a| a == "/tmp/my shots/a b.png"),
            "path was split or mangled: {args:?}"
        );
    }

    /// On the platforms that cannot select a file, revealing must still land in
    /// the right directory rather than trying to "open" the image twice.
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn elsewhere_the_reveal_opens_the_containing_directory() {
        let (_, args) = reveal_command(Path::new("/tmp/shots/a.png"));
        assert_eq!(args, vec!["/tmp/shots".to_string()]);
    }
}
