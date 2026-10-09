// Prints the live level of each Sonar channel for 8 seconds (needs something playing on a channel).
fn main() {
    sonar_lite::levels::spawn();
    for _ in 0..16 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        println!("game {:.3} chat {:.3} media {:.3} aux {:.3}", sonar_lite::levels::get("game"), sonar_lite::levels::get("chat"), sonar_lite::levels::get("media"), sonar_lite::levels::get("aux"));
    }
}
