//! `kaos-visual` — the mandala editor, as its own application.
//!
//! Draw the `o-[]-o` notation and it writes Rebis; open a program and it draws
//! it. Tabs hold drawings, source, conversations, or the sigil library, all
//! read from the same `~/.kaos` the terminal app uses — but nothing here
//! depends on that app, and it runs without it.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return;
    }
    let arg = args.join(" ");
    match kaos_visual::open(&arg) {
        Ok(mandala) => kaos_visual::run(mandala),
        Err(error) => {
            eprintln!("kaos-visual: {error}");
            std::process::exit(2);
        }
    }
}

const USAGE: &str = "\
kaos-visual — the mandala editor

    kaos-visual                    an empty canvas
    kaos-visual program.rebis      draw a saved program
    kaos-visual '(-> \"a\" \"b\")'     draw inline source

Tabs:  Ctrl-T new drawing   Ctrl-Tab cycle   Ctrl-W close
Canvas: drag to pan, wheel to zoom, Delete removes the selection.
Sigils and conversations are read from ~/.kaos, shared with the kaos
terminal app; neither requires the other to be installed.";
