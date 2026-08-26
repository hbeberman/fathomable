//! Print a Markdown file's layout to stdout for eyeballing.
use fathomable_core::layout::Layout;
fn main() {
    let path = std::env::args().nth(1).unwrap_or_default();
    let width: usize = std::env::args()
        .nth(2)
        .and_then(|w| w.parse().ok())
        .unwrap_or(60);
    let text = std::fs::read_to_string(path).unwrap_or_default();
    for line in Layout::render(&text, width).lines() {
        println!(
            "{:>4} {}",
            line.source_line().map_or(String::new(), |n| n.to_string()),
            line.text()
        );
        if std::env::var("SPANS").is_ok() {
            println!("     {:?}", line.spans());
        }
    }
}
