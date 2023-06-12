use tokio;

use lib::distro;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();

    distro::create_user(args[1].as_str()).await;

    return;
}
