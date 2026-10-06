fn main() {
    margins_cli::logging::init_stderr_logger();
    std::process::exit(margins_cli::main_entry_from_env());
}
