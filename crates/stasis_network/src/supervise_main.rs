fn main() {
    let result = stasis_network::supervision::parse_args(std::env::args().skip(1))
        .and_then(|options| stasis_network::supervision::run(&options));
    if let Err(error) = result {
        eprintln!("stasis-network-supervise: {error}");
        std::process::exit(1);
    }
}
