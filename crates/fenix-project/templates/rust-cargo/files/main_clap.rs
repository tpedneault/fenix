use clap::Parser;

/// {{name}}
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Who to greet
    #[arg(default_value = "world")]
    name: String,

    /// Say it louder
    #[arg(long)]
    shout: bool,
}

fn main() {
    let args = Args::parse();
    let text = format!("Hello, {}!", args.name);
    println!("{}", if args.shout { text.to_uppercase() } else { text });
}
