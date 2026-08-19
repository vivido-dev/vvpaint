//! Real Vivido smoke coverage for vvpaint.
//!
//! This is ignored by default because it spawns the presenter and requires a usable wgpu adapter.
//! Run it after building Vivido with:
//!
//! ```text
//! cargo test --test headless -- --ignored --test-threads=1
//! ```

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(30);

struct Session {
    vivido: PathBuf,
    runtime: PathBuf,
    socket: String,
}

impl Session {
    fn start(export: &Path) -> Self {
        let vivido = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../vivido/target/debug")
            .join(format!("vivido{}", std::env::consts::EXE_SUFFIX));
        assert!(
            vivido.is_file(),
            "build the sibling Vivido debug binary first"
        );

        let runtime = std::env::temp_dir().join(format!("vvpaint-headless-{}", std::process::id()));
        let _ = fs::remove_dir_all(&runtime);
        fs::create_dir_all(&runtime).expect("create private headless runtime");

        let program = PathBuf::from(env!("CARGO_BIN_EXE_vvpaint"));
        let mut command = base_command(&vivido, &runtime);
        command.args([
            "--headless",
            "--session",
            "vvpaint-smoke",
            "--headless-size",
            "800x500px",
            "--hold",
        ]);
        command
            .arg("-e")
            .arg(program)
            .args(["--output", export.to_str().expect("UTF-8 export path")]);
        let output = command.output().expect("launch headless Vivido");
        assert!(
            output.status.success(),
            "Vivido startup failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let socket = stdout
            .lines()
            .find_map(|line| line.strip_prefix("VIVIDO_SOCKET="))
            .and_then(|value| value.split(';').next())
            .unwrap_or_else(|| panic!("Vivido did not publish an endpoint: {stdout:?}"))
            .to_owned();

        let session = Self {
            vivido,
            runtime,
            socket,
        };
        session.wait_for_text("Pencil");
        session
    }

    fn msg<S: AsRef<OsStr>>(&self, args: &[S]) -> Output {
        let mut command = base_command(&self.vivido, &self.runtime);
        command.args(["msg", "--socket", &self.socket]);
        command.args(args);
        command.output().expect("run Vivido automation command")
    }

    fn checked_msg<S: AsRef<OsStr>>(&self, args: &[S]) -> String {
        let output = self.msg(args);
        assert!(
            output.status.success(),
            "Vivido automation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn wait_for_text(&self, needle: &str) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let output = self.msg(&["get-text"]);
            let response = if output.status.success() {
                String::from_utf8_lossy(&output.stdout).into_owned()
            } else {
                String::from_utf8_lossy(&output.stderr).into_owned()
            };
            if output.status.success() && response.contains(needle) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "vvpaint never displayed {needle:?}; last response: {response:?}"
            );
            thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.msg(&["quit"]);
        thread::sleep(Duration::from_millis(200));
        let _ = fs::remove_dir_all(&self.runtime);
    }
}

fn base_command(vivido: &Path, runtime: &Path) -> Command {
    let mut command = Command::new(vivido);
    #[cfg(windows)]
    command.env("VIVIDO_RUNTIME_DIR", runtime);
    #[cfg(unix)]
    command.env("XDG_RUNTIME_DIR", runtime);
    command
        .env_remove("VIVIDO_SOCKET")
        .env_remove("VIVIDO_SESSION");
    command
}

fn assert_nonblank_png(path: &Path) {
    let image = image::open(path)
        .expect("decode smoke-test PNG")
        .into_rgba8();
    let first = image.get_pixel(0, 0);
    assert!(
        image.pixels().any(|pixel| pixel != first),
        "painted image contains only one color"
    );
}

#[test]
#[ignore = "spawns the real headless Vivido presenter and needs a wgpu adapter"]
fn draws_resizes_presents_and_exports() {
    let export = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("vvpaint-smoke-{}.png", std::process::id()));
    let _ = fs::remove_file(&export);
    let session = Session::start(&export);

    // Select Brush from the first toolbar row and bright red from the second palette row. UI
    // routing converts these stable grid cells through Vivido's normal pixel-mode input path.
    session.checked_msg(&[
        "mouse",
        "down",
        "--cell-column",
        "12",
        "--cell-row",
        "26",
        "--route",
        "ui",
    ]);
    session.checked_msg(&[
        "mouse",
        "up",
        "--cell-column",
        "12",
        "--cell-row",
        "26",
        "--route",
        "ui",
    ]);
    session.wait_for_text("Brush");
    session.checked_msg(&[
        "mouse",
        "down",
        "--cell-column",
        "35",
        "--cell-row",
        "27",
        "--route",
        "ui",
    ]);
    session.checked_msg(&[
        "mouse",
        "up",
        "--cell-column",
        "35",
        "--cell-row",
        "27",
        "--route",
        "ui",
    ]);
    session.wait_for_text("Primary color: red");

    session.checked_msg(&["mouse", "down", "--x", "60", "--y", "60", "--route", "ui"]);
    session.checked_msg(&["mouse", "drag", "--x", "260", "--y", "180", "--route", "ui"]);
    session.checked_msg(&["mouse", "up", "--x", "260", "--y", "180", "--route", "ui"]);
    session.checked_msg(&["resize", "--width", "900", "--height", "600"]);
    thread::sleep(Duration::from_millis(750));

    let screenshot = PathBuf::from(session.checked_msg(&["screenshot"]).trim());
    assert_nonblank_png(&screenshot);
    session.checked_msg(&["key", "q"]);

    let deadline = Instant::now() + TIMEOUT;
    while !export.is_file() {
        assert!(Instant::now() < deadline, "vvpaint did not export after q");
        thread::sleep(Duration::from_millis(100));
    }
    assert_nonblank_png(&export);

    let _ = fs::remove_file(screenshot);
    let _ = fs::remove_file(export);
}
