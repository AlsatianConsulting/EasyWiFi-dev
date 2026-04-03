use anyhow::Result;

fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("--test-wifi") => easywifi::test_mode::run_wifi_cli(&args[1..]),
        Some("--test-bluetooth") => easywifi::test_mode::run_bluetooth_cli(&args[1..]),
        Some("--help") | Some("-h") => {
            print_usage();
            Ok(())
        }
        Some(other) => anyhow::bail!("unknown argument: {}", other),
        None => easywifi::webui::run(),
    }
}

fn print_usage() {
    println!("EasyWiFi");
    println!();
    println!("Usage:");
    println!("  easywifi");
    println!("  easywifi --test-wifi --interface <iface> [options]");
    println!("  easywifi --test-bluetooth [options]");
    println!();
    easywifi::test_mode::print_wifi_test_usage();
    println!();
    easywifi::test_mode::print_bluetooth_test_usage();
}
