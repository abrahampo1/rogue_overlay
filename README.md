# Rogue Overlay

An overlay for **Rogue Lineage** (Roblox), written in Rust. It is the successor to RogueCompanion.

- **Mana bar skins**: the game still draws its own mana bar. The overlay puts a transparent frame or ruler on top of it so you can read the percentage at a glance. It ships 8 built-in styles (ruler every 10 %, every 25 %, frames…), the example `ruler.png`, and supports your own images with transparency (png, animated gif, webp). It can optionally show the numeric % by reading the mana from the screen.
- **Skill cooldowns** on the hotbar work like RogueCompanion: press the slot key (1–0, -, =), then click in game. The slot darkens with a countdown and flashes when it is ready again. A skill library stores names and cooldowns so you can assign them quickly.
- **Ore timer**: sync it when you see the ores spawn (F5 or the panel button). It then counts down 10 min in a loop, with a warning colour and a sound.
- **Control panel** to configure everything, and an **edit mode** to place every element with the mouse in game.

The overlay only *observes* the keyboard and mouse through low-level hooks. It never sends input and never touches the Roblox process. It is a transparent, always-on-top, click-through window.

## Build

```powershell
cargo build --release
```

The executable ends up in `target\release\rogue-overlay.exe`. `config.json` and the `skins\` folder are created next to it.

## Usage

1. Open Roblox first (window titled `Roblox`, ideally fullscreen), then `rogue-overlay.exe`.
2. If something does not line up with your resolution or UI, press **F6** (or "Place in game" in the panel):
   - drag the mana bar (magenta), the sample strip (yellow), the hotbar (cyan) or the ore timer (green);
   - wheel or PageUp/PageDown changes the height, slot spacing or text size; with **Ctrl** it changes the width or slot size;
   - arrows move the selected element 1 px (10 px with Shift);
   - **C** with the cursor over the EMPTY part of the in-game bar picks that colour for the numeric %;
   - **Esc**, **Enter**, **F6** or the **Done** button saves and exits.

| Key | Action |
|---|---|
| F5 | Sync the ore timer (with Roblox in the foreground) |
| F6 | Edit mode |
| F7 | Calibration (shows the boxes without editing) |
| F8 | Show / hide |
| F9 | Reload `config.json` |
| F10 | Quit |

All keys can be changed in Settings.

## Default measurements (1920×1080)

Coordinates are relative to the Roblox client area. A negative value is measured from the right or bottom edge.

- **Hotbar**: 60 px slots, 72 px apart, centred with a −2 px offset. These are the same measurements as RogueCompanion's `skilloverlay`. The top edge sits 73 px from the bottom. The game centres the hotbar and adds slots as you learn skills, so the overlay counts the slots on screen every half second by looking for the slot body colour (≈ `#D7CBBF`). You can also fix the number in the Skill cooldowns tab. The selected slot is also read from the screen (the game lights it up), so the overlay never gets out of sync with the game; key presses are only a fallback when the hotbar is not visible.
- **Mana**: vertical bar on the left, 34×356 frame at (20, 553). The skin is stretched to that box.
- **Numeric %** (optional): a 6 px strip in the middle of the bar (y 561–903) is sampled. From the bottom up, every row that is *not* the empty-bar colour (≈ `#524048`, ±22) counts as mana. The mana itself can be any colour (purple, blue…).

The measurements come from 1920×1080 screenshots. If the numeric % is wrong (for example, stuck at 0 % or 100 %), enter F6 and press **C** over the empty part of the bar to pick its colour.

For the % to work, the overlay hides itself from screen captures (Settings → "Hide the overlay from captures / OBS"). Otherwise the reading would see the skin drawn on top. The trade-off is that OBS and Discord will not show the overlay. If you want to record it, turn that option off and do not use the numeric %.

Screen reads cost a little game performance, so their rate is configurable: mana reads/s (default 20) in the Mana skins tab and hotbar reads/s (default 10) in the Skill cooldowns tab. Both are capped by the overlay FPS in Settings.

## Skill auto-detection and the community catalog

With "Auto-detect skills from the hotbar" on (Skill cooldowns tab), the overlay reads the names the game draws on each hotbar slot. It uses the OCR built into Windows 10/11, on a background thread, when the hotbar changes, every 8 s, or with "Detect now". Each name is matched (fuzzy, so OCR slips like `I gnis` still work) first against your own skill library, then against the community catalog, and the slot gets its name and cooldown. A skill whose cooldown is not known gets 0 s, which means it is not tracked. Slots you edit by hand are marked as yours and never overwritten.

The catalog lives in [`catalog/skills.json`](catalog/skills.json). "Update from GitHub" in the panel downloads the latest version, so fixes reach everyone without a new release. Each entry looks like this:

```json
{ "name": "The Soul", "cooldown": 30, "verified": true, "source": "https://rogue-lineage.fandom.com/wiki/Whisperer" }
```

- `cooldown`: seconds, or `null` if unknown.
- `verified`: `true` once a person has checked the value. The first values were extracted automatically from the [Rogue Lineage wiki](https://rogue-lineage.fandom.com/) and may be wrong; they are marked with `*` in the panel.
- `aliases`: other spellings, for example how the hotbar wraps a long name.
- `mana` / `snap_mana`: mana % range needed to cast a spell.
- `notes`, `source`: free text and where the value comes from.

To contribute, fix or add entries and open a pull request.

### Spell cast window

When the selected hotbar slot is a spell with a known mana range, the overlay marks it on the mana bar. The normal cast range is a green band on the left half and the snap range a blue band on the right half, with labels next to the bar. With the numeric % on, the band your mana is currently in lights up. Toggle it with "Mark the selected spell's mana window" in the Mana skins tab.

## Custom skins

Use "Add image…" in the panel, or copy images into `skins\`. The image is stretched to the mana bar box and drawn **on top** of the game's bar. It needs transparency wherever the mana should show through: `ruler.png`, for example, is only a frame and marks. Animated gifs play.

## Layout

| File | Purpose |
|---|---|
| `src/main.rs` | Startup: launches the overlay thread and the panel |
| `src/overlay.rs` | Overlay logic: cooldowns, mana skin, ore timer, edit mode, drawing |
| `src/panel.rs` | egui control panel |
| `src/win.rs` | Win32: layered window, hooks, screen capture, sound |
| `src/render.rs` | Text (fontdue), image skins and tiny-skia helpers |
| `src/skins.rs` | Catalog: built-in styles and images from `skins\` |
| `src/config.rs` | `config.json` and defaults |
| `src/shared.rs` | State shared between the panel and the overlay |
