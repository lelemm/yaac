//! The SSH askpass helper on its own (`yaac` itself is the helper in a real install; this
//! binary lets the hub's tests run setup without the window's executable). See
//! `yaac_hub::ssh::askpass`.

fn main() -> std::process::ExitCode {
    yaac_hub::ssh::askpass::client_main().unwrap_or(std::process::ExitCode::FAILURE)
}
