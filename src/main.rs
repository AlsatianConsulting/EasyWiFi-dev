use anyhow::Result;

fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("--test-wifi") => simplestg::test_mode::run_wifi_cli(&args[1..]),
        Some("--help") | Some("-h") => {
            print_usage();
            Ok(())
        }
        Some(other) => anyhow::bail!("unknown argument: {}", other),
        None => simplestg::ui::run(),
    }
}

fn print_usage() {
    println!("SimpleSTG");
    println!();
    println!("Usage:");
    println!("  simplestg");
    println!("  simplestg --test-wifi --interface <iface> [options]");
    println!();
    simplestg::test_mode::print_wifi_test_usage();
}
