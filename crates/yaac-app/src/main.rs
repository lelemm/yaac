//! `yaac`: the window, or `yaac hub`, the headless hub. See [`yaac_app::run`].

fn main() -> std::process::ExitCode {
    yaac_app::run(&std::env::args().skip(1).collect::<Vec<_>>())
}
