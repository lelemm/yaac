use yaac_preview::{BrowserHost, Preview};

struct Demo {
    preview: Preview,
}
impl eframe::App for Demo {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        self.preview.ui(ui);
    }
}
pub(crate) fn run(args: &[String]) -> std::process::ExitCode {
    let (url, socks) = match args {
        [url] => (url, None),
        [url, flag, port] if flag == "--socks" => match port.parse::<u16>() {
            Ok(p) if p != 0 => (url, Some(p)),
            _ => {
                eprintln!("invalid SOCKS port");
                return 2.into();
            }
        },
        _ => {
            eprintln!("usage: yaac --demo preview <url> [--socks <port>]");
            return 2.into();
        }
    };
    let result = eframe::run_native(
        "YAAC web preview",
        Default::default(),
        Box::new(move |cc| {
            Ok(Box::new(Demo {
                preview: Preview::new(&mut BrowserHost::default(), url, socks, &cc.egui_ctx),
            }))
        }),
    );
    if let Err(e) = result {
        eprintln!("{e}");
        return 1.into();
    }
    std::process::ExitCode::SUCCESS
}
