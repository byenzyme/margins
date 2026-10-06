fn main() -> anyhow::Result<()> {
    margins_server::logging::init_stderr_logger();
    margins_server::run()
}
