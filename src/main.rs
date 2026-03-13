use anyhow::Result;

fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("--test-wifi") => wirelessexplorer::test_mode::run_wifi_cli(&args[1..]),
        Some("--test-bluetooth") => wirelessexplorer::test_mode::run_bluetooth_cli(&args[1..]),
        Some("--test-sdr") => wirelessexplorer::test_mode::run_sdr_cli(&args[1..]),
        Some("--help") | Some("-h") => {
            print_usage();
            Ok(())
        }
        Some(other) => anyhow::bail!("unknown argument: {}", other),
        None => wirelessexplorer::ui::run(),
    }
}

fn print_usage() {
    println!("WirelessExplorer");
    println!();
    println!("Usage:");
    println!("  wirelessexplorer");
    println!("  wirelessexplorer --test-wifi --interface <iface> [options]");
    println!("  wirelessexplorer --test-bluetooth [options]");
    println!("  wirelessexplorer --test-sdr [options]");
    println!();
    wirelessexplorer::test_mode::print_wifi_test_usage();
    println!();
    wirelessexplorer::test_mode::print_bluetooth_test_usage();
    println!();
    wirelessexplorer::test_mode::print_sdr_test_usage();
}
