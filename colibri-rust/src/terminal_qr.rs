use qrcodegen::{QrCode, QrCodeEcc};

pub(crate) fn qr_matrix(text: &str) -> Option<Vec<String>> {
    let qr = QrCode::encode_text(text, QrCodeEcc::Low).ok()?;
    Some(
        (0..qr.size())
            .map(|y| {
                (0..qr.size())
                    .map(|x| if qr.get_module(x, y) { '1' } else { '0' })
                    .collect()
            })
            .collect(),
    )
}

/// qrcode-terminal 0.12.0 small-mode layout, as used by the official Feishu tools.
/// Rust adaptation of https://github.com/gtanner/qrcode-terminal (Apache-2.0).
/// Black modules use the terminal background; white modules use its foreground.
/// The graphical handheld view retains its separate four-module quiet zone.
pub fn render_terminal_qr(text: &str) -> Option<String> {
    let matrix = qr_matrix(text)?;
    let size = matrix.len();
    let mut lines = vec!["▄".repeat(size + 2)];
    for y in (0..size).step_by(2) {
        let mut line = String::from("█");
        for x in 0..size {
            let top = matrix[y].as_bytes()[x] == b'1';
            let bottom = y + 1 < size && matrix[y + 1].as_bytes()[x] == b'1';
            line.push(match (top, bottom) {
                (false, false) => '█',
                (false, true) => '▀',
                (true, false) => '▄',
                (true, true) => ' ',
            });
        }
        line.push('█');
        lines.push(line);
    }
    // QR Model 2 matrices are always odd-sized: the final paired row is white.
    Some(lines.join("\n"))
}
