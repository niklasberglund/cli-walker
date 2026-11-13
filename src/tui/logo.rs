//! The block-font "CLI WALKER" logo, coloured with a purple-to-pink gradient.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// 77 columns wide, centred over an 80-column budget. Letters sit flush against each other
/// so the gap between the two words still reads as a space.
const ART: [&str; 6] = [
    "    ██████╗██╗     ██╗     ██╗    ██╗ █████╗ ██╗     ██╗  ██╗███████╗██████╗",
    "   ██╔════╝██║     ██║     ██║    ██║██╔══██╗██║     ██║ ██╔╝██╔════╝██╔══██╗",
    "   ██║     ██║     ██║     ██║ █╗ ██║███████║██║     █████╔╝ █████╗  ██████╔╝",
    "   ██║     ██║     ██║     ██║███╗██║██╔══██║██║     ██╔═██╗ ██╔══╝  ██╔══██╗",
    "   ╚██████╗███████╗██║     ╚███╔███╔╝██║  ██║███████╗██║  ██╗███████╗██║  ██║",
    "    ╚═════╝╚══════╝╚═╝      ╚══╝╚══╝ ╚═╝  ╚═╝╚══════╝╚═╝  ╚═╝╚══════╝╚═╝  ╚═╝",
];

const LOGO_WIDTH: u16 = 80;
pub const HEIGHT: u16 = ART.len() as u16;

const GRADIENT_START: (u8, u8, u8) = (101, 78, 163);
const GRADIENT_END: (u8, u8, u8) = (234, 175, 200);

/// The colour `t` of the way (0..=1) from the gradient's purple end to its pink end.
pub fn gradient(t: f64) -> Color {
    let t = t.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * t) as u8;
    Color::Rgb(
        mix(GRADIENT_START.0, GRADIENT_END.0),
        mix(GRADIENT_START.1, GRADIENT_END.1),
        mix(GRADIENT_START.2, GRADIENT_END.2),
    )
}

pub fn lines() -> Vec<Line<'static>> {
    ART.iter()
        .map(|row| {
            Line::from(
                row.chars()
                    .enumerate()
                    .map(|(column, ch)| {
                        let t = column as f64 / f64::from(LOGO_WIDTH - 1);
                        Span::styled(ch.to_string(), Style::new().fg(gradient(t)))
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}
