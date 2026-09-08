use colored::*;
use clap::Parser;
use serde::{Deserialize, Serialize};

#[derive(Parser, Debug, Clone)]
pub struct Args {
    #[clap(short, long)]
    pub threads: Option<usize>,
    #[clap(short, long)]
    pub address: Option<String>,
    #[clap(short, long)]
    pub pool: Option<String>,
    /// Light mode: ~256 MB instead of a 2.3 GB dataset, but roughly 10x slower.
    #[clap(long, default_value_t = false)]
    pub light: bool,
}

impl Args {
    pub fn parse_and_validate() -> Args {
        let args = Args::parse();
        if args.address.is_none() || args.pool.is_none() {
            Args::show_demo_usage();
            std::process::exit(0);
        }
        args
    }

    pub fn show_demo_usage() {
        println!();
        println!("{}", "Run the miner with required arguments:".bold().bright_yellow());
        println!("{}", "--address <shaicoin_address> --pool <POOL_URL>".bold().bright_red());
        println!("{}", "OPTIONAL: --threads <AMT>".bold().bright_red());
        println!("{}", "OPTIONAL: --light  (256 MB instead of 2.3 GB, ~10x slower)".bold().bright_red());
        println!();
        println!("Example:");
        println!("./shaipot --address sh1q... --pool wss://mine.shaicoin-mining.com/ --threads 4");
        println!();
        println!("{}", "Full speed wants huge pages on the host:".bold().bright_yellow());
        println!("  sudo sysctl -w vm.nr_hugepages=1280");
    }
}

#[derive(Serialize, Deserialize)]
pub struct SubmitMessage {
    pub r#type: String,
    pub miner_id: String,
    pub nonce: String,
    pub job_id: String,
}

#[derive(Deserialize, Debug)]
pub struct ServerMessage {
    pub r#type: String,
    pub job_id: Option<String>,
    pub data: Option<String>,
    /// 60-byte RandomX key, hex. Added by the RandomX fork; a pool that does
    /// not send it cannot be mined.
    pub seed: Option<String>,
    pub target: Option<String>,
    pub message: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Job {
    pub job_id: String,
    pub data: String,
    pub seed: String,
    pub target: String,
}
