fn main() -> std::process::ExitCode {
    if !mailroom::disable_core_dumps() {
        println!("status=core_dumps_enabled");
        return std::process::ExitCode::from(70);
    }
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(target_os = "macos")]
    if matches!(
        arguments.first().map(String::as_str),
        Some("keychain" | "native-check")
    ) {
        return std::process::ExitCode::from(mailroom::owner::run(&arguments));
    }
    let _ = arguments;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build();
    match runtime.map(|r| r.block_on(mailroom::server::run())) {
        Ok(Ok(())) => std::process::ExitCode::SUCCESS,
        _ => std::process::ExitCode::FAILURE,
    }
}
