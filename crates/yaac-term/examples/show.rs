//! Replays a fixture and prints the screen: `show <file.vt> <cols> <rows>`.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let bytes = std::fs::read(&a[0]).expect("fixture");
    let mut term = yaac_term::Terminal::new(a[1].parse().unwrap_or(80), a[2].parse().unwrap_or(24), || {});
    term.resize(a[1].parse().unwrap_or(80), a[2].parse().unwrap_or(24));
    term.feed(&bytes);
    while !term.idle() {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let mut f = yaac_term::Frame::default();
    term.snapshot(&mut f);
    for (i, l) in f.lines.iter().enumerate() {
        println!("{i:>2}|{}|", l.text());
    }
    println!("cursor {:?} modes {:?} title {:?}", f.cursor, f.modes, f.title);
}
