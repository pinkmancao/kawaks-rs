use std::{
    fs, io,
    path::{Path, PathBuf},
};

use kawaks_rs::Dino;
use sdl2::{
    event::{Event, WindowEvent},
    keyboard::Keycode,
    mouse::MouseButton,
    pixels::Color,
    rect::{Point, Rect},
};

use super::{box_color, i18n::Language, text, ACCENT, INK, MUTED, PANEL};

pub(super) enum Start {
    Default,
    Stage(u8),
    Checkpoint(PathBuf),
}

const WIDTH: u32 = 720;
const HEIGHT: u32 = 474;
const PAGE: usize = 18;

fn row_rect(index: usize) -> Rect {
    Rect::new(
        32 + (index / 9) as i32 * 332,
        121 + (index % 9) as i32 * 31,
        324,
        27,
    )
}

fn choices(game: &str, directory: &Path, lang: Language) -> Vec<(String, Start)> {
    let mut choices = vec![(lang.tr("Normal start").to_owned(), Start::Default)];
    choices.extend(
        Dino::starting_stages(game)
            .iter()
            .enumerate()
            .map(|(i, name)| (lang.tr(name).to_owned(), Start::Stage(i as u8))),
    );
    let prefix = format!("{game}-stage-");
    let mut saves = Vec::new();
    if let Ok(entries) = fs::read_dir(directory) {
        for file in entries.flatten() {
            let path = file.path();
            if !path.is_file() {
                continue;
            }
            let Some((stem, extension)) = path
                .file_stem()
                .and_then(|name| name.to_str())
                .zip(path.extension().and_then(|ext| ext.to_str()))
            else {
                continue;
            };
            let archive_id = stem.to_ascii_lowercase();
            let extension = extension.to_ascii_lowercase();
            let slot = extension
                .strip_prefix("ks")
                .and_then(|value| value.parse::<u8>().ok())
                .filter(|&number| number < 10);
            let label = if archive_id == game {
                slot.map(|number| format!("{} {number}", lang.tr("Saved slot")))
            } else if ["kwk", "kwk4"].contains(&extension.as_str()) || slot.is_some() {
                archive_id
                    .strip_prefix(&prefix)
                    .filter(|name| !name.is_empty())
                    .map(|name| format!("{} {}", lang.tr("Checkpoint"), name.replace('-', " ")))
            } else {
                None
            };
            if let Some(label) = label {
                saves.push((label, Start::Checkpoint(path)));
            }
        }
    }
    saves.sort_by(|(a, _), (b, _)| a.cmp(b));
    choices.extend(saves);
    choices
}

pub(super) fn run(
    sdl: &sdl2::Sdl,
    events: &mut sdl2::EventPump,
    game: &str,
    directory: &Path,
    lang: Language,
) -> io::Result<Option<Start>> {
    let choices = choices(game, directory, lang);
    let pages = choices.len().div_ceil(PAGE);
    let video = sdl.video().map_err(io::Error::other)?;
    let mut canvas = video
        .window(lang.tr("Kawaks Rust - Starting Stage"), WIDTH, HEIGHT)
        .position_centered()
        .resizable()
        .build()
        .map_err(io::Error::other)?
        .into_canvas()
        .build()
        .map_err(io::Error::other)?;
    canvas
        .set_logical_size(WIDTH, HEIGHT)
        .map_err(io::Error::other)?;
    let window = canvas.window().id();
    let previous = Rect::new(546, 415, 62, 30);
    let next = Rect::new(626, 415, 62, 30);
    let mut page = 0usize;
    let mut selection = None;

    'picker: loop {
        let event = events.wait_event_timeout(25);
        for event in event.into_iter().chain(events.poll_iter()) {
            match event {
                Event::Quit { .. } => break 'picker,
                Event::Window {
                    window_id,
                    win_event: WindowEvent::Close,
                    ..
                }
                | Event::KeyDown {
                    window_id,
                    keycode: Some(Keycode::Escape),
                    ..
                } if window_id == window => break 'picker,
                Event::MouseWheel { window_id, y, .. } if window_id == window && y != 0 => {
                    page = if y > 0 {
                        page.saturating_sub(1)
                    } else {
                        (page + 1).min(pages - 1)
                    };
                }
                Event::MouseButtonDown {
                    window_id,
                    mouse_btn: MouseButton::Left,
                    x,
                    y,
                    ..
                } if window_id == window => {
                    let point = Point::new(x, y);
                    if previous.contains_point(point) {
                        page = page.saturating_sub(1);
                    } else if next.contains_point(point) {
                        page = (page + 1).min(pages - 1);
                    } else if let Some(index) = (0..PAGE).find(|&index| {
                        page * PAGE + index < choices.len() && row_rect(index).contains_point(point)
                    }) {
                        selection = Some(page * PAGE + index);
                        break 'picker;
                    }
                }
                _ => {}
            }
        }
        canvas.set_draw_color(Color::RGB(14, 23, 34));
        canvas.clear();
        text(&mut canvas, 32, 27, lang.tr("STARTING STAGE"), 2, INK, 26)
            .map_err(io::Error::other)?;
        let hint = if Dino::starting_stages(game).is_empty() {
            "Choose a saved checkpoint, or start normally."
        } else {
            "Select a stage, then coin, start and choose your fighter."
        };
        text(&mut canvas, 32, 70, lang.tr(hint), 1, MUTED, 81).map_err(io::Error::other)?;
        for (index, (label, start)) in choices.iter().skip(page * PAGE).take(PAGE).enumerate() {
            let rect = row_rect(index);
            box_color(&mut canvas, rect, PANEL).map_err(io::Error::other)?;
            text(
                &mut canvas,
                rect.x() + 9,
                rect.y() + 10,
                label,
                1,
                if matches!(start, Start::Checkpoint(_)) {
                    MUTED
                } else {
                    INK
                },
                36,
            )
            .map_err(io::Error::other)?;
        }
        text(
            &mut canvas,
            32,
            426,
            &format!(
                "{}   {}/{}   {}",
                game.to_ascii_uppercase(),
                page + 1,
                pages,
                lang.tr("Esc closes")
            ),
            1,
            ACCENT,
            50,
        )
        .map_err(io::Error::other)?;
        for (button, label) in [(previous, "<"), (next, ">")] {
            box_color(&mut canvas, button, PANEL).map_err(io::Error::other)?;
            text(&mut canvas, button.x() + 26, 426, label, 1, ACCENT, 1)
                .map_err(io::Error::other)?;
        }
        canvas.present();
    }
    Ok(selection.and_then(|index| choices.into_iter().nth(index).map(|(_, start)| start)))
}
