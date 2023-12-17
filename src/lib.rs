//! A platform integration to use [egui](https://github.com/emilk/egui) with [winit](https://github.com/rust-windowing/winit).
//!
//! You need to create a [`Platform`] and feed it with `winit::event::Event` events.
//! Use `begin_frame()` and `end_frame()` to start drawing the egui UI.
//! A basic usage example can be found [here](https://github.com/hasenbanck/egui_example).
#![warn(missing_docs)]

use std::collections::HashMap;

#[cfg(feature = "clipboard")]
use copypasta::{ClipboardContext, ClipboardProvider};
use egui::{
    emath::{pos2, vec2},
    Context, Pos2,
};
use winit::{
    dpi::PhysicalSize,
    event::{Event, TouchPhase, WindowEvent::*, Ime},
    window::CursorIcon, keyboard::{ModifiersState, self, NamedKey, Key, SmolStr},
};

/// Configures the creation of the `Platform`.
#[derive(Debug, Default)]
pub struct PlatformDescriptor {
    /// Width of the window in physical pixel.
    pub physical_width: u32,
    /// Height of the window in physical pixel.
    pub physical_height: u32,
    /// HiDPI scale factor.
    pub scale_factor: f64,
    /// Egui font configuration.
    pub font_definitions: egui::FontDefinitions,
    /// Egui style configuration.
    pub style: egui::Style,
}

#[cfg(feature = "webbrowser")]
fn handle_links(output: &egui::PlatformOutput) {
    if let Some(open_url) = &output.open_url {
        // This does not handle open_url.new_tab
        // webbrowser does not support web anyway
        if let Err(err) = webbrowser::open(&open_url.url) {
            eprintln!("Failed to open url: {}", err);
        }
    }
}

#[cfg(feature = "clipboard")]
fn handle_clipboard(output: &egui::PlatformOutput, clipboard: Option<&mut ClipboardContext>) {
    if !output.copied_text.is_empty() {
        if let Some(clipboard) = clipboard {
            if let Err(err) = clipboard.set_contents(output.copied_text.clone()) {
                eprintln!("Copy/Cut error: {}", err);
            }
        }
    }
}

/// Provides the integration between egui and winit.
pub struct Platform {
    scale_factor: f64,
    context: Context,
    raw_input: egui::RawInput,
    modifier: ModifiersState,
    pointer_pos: Option<egui::Pos2>,

    #[cfg(feature = "clipboard")]
    clipboard: Option<ClipboardContext>,

    // For emulating pointer events from touch events we merge multi-touch
    // pointers, and ref-count the press state.
    touch_pointer_pressed: u32,

    // Egui requires unique u64 device IDs for touch events but Winit's
    // device IDs are opaque, so we have to create our own ID mapping.
    device_indices: HashMap<winit::event::DeviceId, u64>,
    next_device_index: u64,
}

impl Platform {
    /// Creates a new `Platform`.
    pub fn new(descriptor: PlatformDescriptor) -> Self {
        let context = Context::default();

        context.set_fonts(descriptor.font_definitions.clone());
        context.set_style(descriptor.style);
        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::default(),
                vec2(
                    descriptor.physical_width as f32,
                    descriptor.physical_height as f32,
                ) / descriptor.scale_factor as f32,
            )),
            ..Default::default()
        };

        Self {
            scale_factor: descriptor.scale_factor,
            context,
            raw_input,
            modifier: winit::keyboard::ModifiersState::empty(),
            pointer_pos: Some(Pos2::default()),
            #[cfg(feature = "clipboard")]
            clipboard: ClipboardContext::new().ok(),
            touch_pointer_pressed: 0,
            device_indices: HashMap::new(),
            next_device_index: 1,
        }
    }

    /// Handles the given winit event and updates the egui context. Should be called before starting a new frame with `start_frame()`.
    pub fn handle_event<T>(&mut self, winit_event: &Event<T>) {
        match winit_event {
            Event::WindowEvent {
                window_id: _window_id,
                event,
            } => match event {
                // Resize with 0 width and height is used by winit to signal a minimize event on Windows.
                // See: https://github.com/rust-windowing/winit/issues/208
                // There is nothing to do for minimize events, so it is ignored here. This solves an issue where
                // egui window positions would be changed when minimizing on Windows.
                Resized(PhysicalSize {
                    width: 0,
                    height: 0,
                }) => {}
                Resized(physical_size) => {
                    self.raw_input.screen_rect = Some(egui::Rect::from_min_size(
                        Default::default(),
                        vec2(physical_size.width as f32, physical_size.height as f32)
                            / self.scale_factor as f32,
                    ));
                }
                ScaleFactorChanged {
                    scale_factor,
                    inner_size_writer: _,
                } => {
                    self.scale_factor = *scale_factor;
                }
                MouseInput { state, button, .. } => {
                    if let winit::event::MouseButton::Other(..) = button {
                    } else {
                        // push event only if the cursor is inside the window
                        if let Some(pointer_pos) = self.pointer_pos {
                            self.raw_input.events.push(egui::Event::PointerButton {
                                pos: pointer_pos,
                                button: match button {
                                    winit::event::MouseButton::Left => egui::PointerButton::Primary,
                                    winit::event::MouseButton::Right => {
                                        egui::PointerButton::Secondary
                                    }
                                    winit::event::MouseButton::Middle => {
                                        egui::PointerButton::Middle
                                    }
                                    winit::event::MouseButton::Back => {
                                        egui::PointerButton::Extra1
                                    },
                                    winit::event::MouseButton::Forward => {
                                        egui::PointerButton::Extra2
                                    },
                                    winit::event::MouseButton::Other(_) => unreachable!(),
                                },
                                pressed: *state == winit::event::ElementState::Pressed,
                                modifiers: Default::default(),
                            });
                        }
                    }
                }
                Touch(touch) => {
                    let pointer_pos = pos2(
                        touch.location.x as f32 / self.scale_factor as f32,
                        touch.location.y as f32 / self.scale_factor as f32,
                    );

                    let device_id = match self.device_indices.get(&touch.device_id) {
                        Some(id) => *id,
                        None => {
                            let device_id = self.next_device_index;
                            self.device_indices.insert(touch.device_id, device_id);
                            self.next_device_index += 1;
                            device_id
                        }
                    };
                    let egui_phase = match touch.phase {
                        TouchPhase::Started => egui::TouchPhase::Start,
                        TouchPhase::Moved => egui::TouchPhase::Move,
                        TouchPhase::Ended => egui::TouchPhase::End,
                        TouchPhase::Cancelled => egui::TouchPhase::Cancel,
                    };

                    let force = match touch.force {
                        Some(winit::event::Force::Calibrated { force, .. }) => force as f32,
                        Some(winit::event::Force::Normalized(force)) => force as f32,
                        None => 0.0f32, // hmmm, egui can't differentiate unsupported from zero pressure
                    };

                    self.raw_input.events.push(egui::Event::Touch {
                        device_id: egui::TouchDeviceId(device_id),
                        id: egui::TouchId(touch.id),
                        phase: egui_phase,
                        pos: pointer_pos,
                        force: Some(force),
                    });

                    // Currently Winit doesn't emulate pointer events based on
                    // touch events but Egui requires pointer emulation.
                    //
                    // For simplicity we just merge all touch pointers into a
                    // single virtual pointer and ref-count the press state
                    // (i.e. the pointer will remain pressed during multi-touch
                    // events until the last pointer is lifted up)

                    let was_pressed = self.touch_pointer_pressed > 0;

                    match touch.phase {
                        TouchPhase::Started => {
                            self.touch_pointer_pressed += 1;
                        }
                        TouchPhase::Ended | TouchPhase::Cancelled => {
                            self.touch_pointer_pressed = match self
                                .touch_pointer_pressed
                                .checked_sub(1)
                            {
                                Some(count) => count,
                                None => {
                                    eprintln!("Pointer emulation error: Unbalanced touch start/stop events from Winit");
                                    0
                                }
                            };
                        }
                        TouchPhase::Moved => {
                            self.raw_input
                                .events
                                .push(egui::Event::PointerMoved(pointer_pos));
                        }
                    }

                    if !was_pressed && self.touch_pointer_pressed > 0 {
                        self.raw_input.events.push(egui::Event::PointerButton {
                            pos: pointer_pos,
                            button: egui::PointerButton::Primary,
                            pressed: true,
                            modifiers: Default::default(),
                        });
                    } else if was_pressed && self.touch_pointer_pressed == 0 {
                        // Egui docs say that the pressed=false should be sent _before_
                        // the PointerGone.
                        self.raw_input.events.push(egui::Event::PointerButton {
                            pos: pointer_pos,
                            button: egui::PointerButton::Primary,
                            pressed: false,
                            modifiers: Default::default(),
                        });
                        self.raw_input.events.push(egui::Event::PointerGone);
                    }
                }
                MouseWheel { delta, .. } => {
                    let mut delta = match delta {
                        winit::event::MouseScrollDelta::LineDelta(x, y) => {
                            let line_height = 8.0; // TODO as in egui_glium
                            vec2(*x, *y) * line_height
                        }
                        winit::event::MouseScrollDelta::PixelDelta(delta) => {
                            vec2(delta.x as f32, delta.y as f32)
                        }
                    };
                    if cfg!(target_os = "macos") {
                        // See https://github.com/rust-windowing/winit/issues/1695 for more info.
                        delta.x *= -1.0;
                    }

                    // The ctrl (cmd on macos) key indicates a zoom is desired.
                    if self.raw_input.modifiers.ctrl || self.raw_input.modifiers.command {
                        self.raw_input
                            .events
                            .push(egui::Event::Zoom((delta.y / 200.0).exp()));
                    } else {
                        self.raw_input.events.push(egui::Event::Scroll(delta));
                    }
                }
                CursorMoved { position, .. } => {
                    let pointer_pos = pos2(
                        position.x as f32 / self.scale_factor as f32,
                        position.y as f32 / self.scale_factor as f32,
                    );
                    self.pointer_pos = Some(pointer_pos);
                    self.raw_input
                        .events
                        .push(egui::Event::PointerMoved(pointer_pos));
                }
                CursorLeft { .. } => {
                    self.pointer_pos = None;
                    self.raw_input.events.push(egui::Event::PointerGone);
                }
                ModifiersChanged(input) => {
                    self.modifier = input.state();
                    self.raw_input.modifiers = winit_to_egui_modifiers(input.state());
                }
                Ime(ime) => {
                    match ime {
                        Ime::Enabled => {
                            self.raw_input.events.push(egui::Event::CompositionStart);
                        },
                        Ime::Preedit(str, _) => {
                            self.raw_input.events.push(egui::Event::CompositionUpdate(str.clone()));
                        },
                        Ime::Commit(str) => {
                            self.raw_input.events.push(egui::Event::CompositionEnd(str.clone()));
                            //Start a new composition as it is not disabled.
                            self.raw_input.events.push(egui::Event::CompositionStart);
                        },
                        Ime::Disabled => {
                            //Just disable with no input.
                            self.raw_input.events.push(egui::Event::CompositionEnd("".to_string()));
                        }
                    };
                },
                KeyboardInput { event, .. } => {
                    let pressed = event.state == winit::event::ElementState::Pressed;

                    if let Some(text) = event.text.as_ref().filter(|s| s.len() > 1) {
                        self.raw_input.events.push(egui::Event::Text(text.to_string()));
                    } else {
                        match event.logical_key {
                            keyboard::Key::Named(NamedKey::Copy) => {
                                self.raw_input.events.push(egui::Event::Copy)
                            },
                            keyboard::Key::Named(NamedKey::Cut) => {
                                self.raw_input.events.push(egui::Event::Cut)
                            },
                            keyboard::Key::Named(NamedKey::Paste) => {
                                #[cfg(feature = "clipboard")]
                                if let Some(ref mut clipboard) = self.clipboard {
                                    if let Ok(contents) = clipboard.get_contents() {
                                        self.raw_input.events.push(egui::Event::Text(contents))
                                    }
                                }
                            }
                            _ => {
                                if let Some(key) = winit_to_egui_key_code(event.logical_key.clone()) {
                                    self.raw_input.events.push(egui::Event::Key {
                                        key,
                                        pressed,
                                        modifiers: winit_to_egui_modifiers(self.modifier),
                                        repeat: false,
                                    });
                                }
                            }
                        }
                    }
                }
                _ => {}
            },
            Event::DeviceEvent { .. } => {}
            _ => {}
        }
    }

    /// Returns `true` if egui should handle the event exclusively. Check this to
    /// avoid unexpected interactions, e.g. a mouse click registering "behind" the UI.
    pub fn captures_event<T>(&self, winit_event: &Event<T>) -> bool {
        match winit_event {
            Event::WindowEvent {
                window_id: _window_id,
                event,
            } => match event {
                Ime(_) | KeyboardInput { .. } | ModifiersChanged(_) => {
                    self.context().wants_keyboard_input()
                }

                MouseWheel { .. } | MouseInput { .. } => self.context().wants_pointer_input(),

                CursorMoved { .. } => self.context().is_using_pointer(),

                Touch { .. } => self.context().is_using_pointer(),

                _ => false,
            },

            _ => false,
        }
    }

    /// Updates the internal time for egui used for animations. `elapsed_seconds` should be the seconds since some point in time (for example application start).
    pub fn update_time(&mut self, elapsed_seconds: f64) {
        self.raw_input.time = Some(elapsed_seconds);
    }

    /// Starts a new frame by providing a new `Ui` instance to write into.
    pub fn begin_frame(&mut self) {
        self.context.begin_frame(self.raw_input.take());
    }

    /// Ends the frame. Returns what has happened as `Output` and gives you the draw instructions
    /// as `PaintJobs`. If the optional `window` is set, it will set the cursor key based on
    /// egui's instructions.
    pub fn end_frame(&mut self, window: Option<&winit::window::Window>) -> egui::FullOutput {
        // otherwise the below line gets flagged by clippy if both clipboard and webbrowser features are disabled
        #[allow(clippy::let_and_return)]
        let output = self.context.end_frame();

        if let Some(window) = window {
            if let Some(cursor_icon) = egui_to_winit_cursor_icon(output.platform_output.cursor_icon)
            {
                window.set_cursor_visible(true);
                // if the pointer is located inside the window, set cursor icon
                if self.pointer_pos.is_some() {
                    window.set_cursor_icon(cursor_icon);
                }
            } else {
                window.set_cursor_visible(false);
            }
        }

        #[cfg(feature = "clipboard")]
        handle_clipboard(&output.platform_output, self.clipboard.as_mut());

        #[cfg(feature = "webbrowser")]
        handle_links(&output.platform_output);

        output
    }

    /// Returns the internal egui context.
    pub fn context(&self) -> Context {
        self.context.clone()
    }

    /// Returns a mutable reference to the raw input that will be passed to egui
    /// the next time [`Self::begin_frame`] is called
    pub fn raw_input_mut(&mut self) -> &mut egui::RawInput {
        &mut self.raw_input
    }
}

/// Translates winit to egui keycodes.
#[inline]
fn winit_to_egui_key_code(key: Key) -> Option<egui::Key> {

    const KEY1: SmolStr = SmolStr::new_inline("1");
    const KEY2: SmolStr = SmolStr::new_inline("2");
    const KEY3: SmolStr = SmolStr::new_inline("3");
    const KEY4: SmolStr = SmolStr::new_inline("4");
    const KEY5: SmolStr = SmolStr::new_inline("5");
    const KEY6: SmolStr = SmolStr::new_inline("6");
    const KEY7: SmolStr = SmolStr::new_inline("7");
    const KEY8: SmolStr = SmolStr::new_inline("8");
    const KEY9: SmolStr = SmolStr::new_inline("9");
    const KEY0: SmolStr = SmolStr::new_inline("0");

    const KEY_A: SmolStr = SmolStr::new_inline("a");
    const KEY_B: SmolStr = SmolStr::new_inline("b");
    const KEY_C: SmolStr = SmolStr::new_inline("c");
    const KEY_D: SmolStr = SmolStr::new_inline("d");
    const KEY_E: SmolStr = SmolStr::new_inline("e");
    const KEY_F: SmolStr = SmolStr::new_inline("f");
    const KEY_G: SmolStr = SmolStr::new_inline("g");
    const KEY_H: SmolStr = SmolStr::new_inline("h");
    const KEY_I: SmolStr = SmolStr::new_inline("i");
    const KEY_J: SmolStr = SmolStr::new_inline("j");
    const KEY_K: SmolStr = SmolStr::new_inline("k");
    const KEY_L: SmolStr = SmolStr::new_inline("l");
    const KEY_M: SmolStr = SmolStr::new_inline("m");
    const KEY_N: SmolStr = SmolStr::new_inline("n");
    const KEY_O: SmolStr = SmolStr::new_inline("o");
    const KEY_P: SmolStr = SmolStr::new_inline("p");
    const KEY_Q: SmolStr = SmolStr::new_inline("q");
    const KEY_R: SmolStr = SmolStr::new_inline("r");
    const KEY_S: SmolStr = SmolStr::new_inline("s");
    const KEY_T: SmolStr = SmolStr::new_inline("t");
    const KEY_U: SmolStr = SmolStr::new_inline("u");
    const KEY_V: SmolStr = SmolStr::new_inline("v");
    const KEY_W: SmolStr = SmolStr::new_inline("w");
    const KEY_X: SmolStr = SmolStr::new_inline("x");
    const KEY_Y: SmolStr = SmolStr::new_inline("y");
    const KEY_Z: SmolStr = SmolStr::new_inline("z");

    Some(match key {
        Key::Named(NamedKey::Escape) => egui::Key::Escape,
        Key::Named(NamedKey::Insert) => egui::Key::Insert,
        Key::Named(NamedKey::Home) => egui::Key::Home,
        Key::Named(NamedKey::Delete) => egui::Key::Delete,
        Key::Named(NamedKey::End) => egui::Key::End,
        Key::Named(NamedKey::PageDown) => egui::Key::PageDown,
        Key::Named(NamedKey::PageUp) => egui::Key::PageUp,
        Key::Named(NamedKey::ArrowLeft) => egui::Key::ArrowLeft,
        Key::Named(NamedKey::ArrowUp) => egui::Key::ArrowUp,
        Key::Named(NamedKey::ArrowRight) => egui::Key::ArrowRight,
        Key::Named(NamedKey::ArrowDown) => egui::Key::ArrowDown,
        Key::Named(NamedKey::Backspace) => egui::Key::Backspace,
        Key::Named(NamedKey::Enter) => egui::Key::Enter,
        Key::Named(NamedKey::Tab) => egui::Key::Tab,
        Key::Named(NamedKey::Space) => egui::Key::Space,
        Key::Character(c) if c == KEY1 => egui::Key::Num1,
        Key::Character(c) if c == KEY2 => egui::Key::Num2,
        Key::Character(c) if c == KEY3 => egui::Key::Num3,
        Key::Character(c) if c == KEY4 => egui::Key::Num4,
        Key::Character(c) if c == KEY5 => egui::Key::Num5,
        Key::Character(c) if c == KEY6 => egui::Key::Num6,
        Key::Character(c) if c == KEY7 => egui::Key::Num7,
        Key::Character(c) if c == KEY8 => egui::Key::Num8,
        Key::Character(c) if c == KEY9 => egui::Key::Num9,
        Key::Character(c) if c == KEY0 => egui::Key::Num0,
        Key::Character(c) if c == KEY_A => egui::Key::A,
        Key::Character(c) if c == KEY_B => egui::Key::B,
        Key::Character(c) if c == KEY_C => egui::Key::C,
        Key::Character(c) if c == KEY_D => egui::Key::D,
        Key::Character(c) if c == KEY_E => egui::Key::E,
        Key::Character(c) if c == KEY_F => egui::Key::F,
        Key::Character(c) if c == KEY_G => egui::Key::G,
        Key::Character(c) if c == KEY_H => egui::Key::H,
        Key::Character(c) if c == KEY_I => egui::Key::I,
        Key::Character(c) if c == KEY_J => egui::Key::J,
        Key::Character(c) if c == KEY_K => egui::Key::K,
        Key::Character(c) if c == KEY_L => egui::Key::L,
        Key::Character(c) if c == KEY_M => egui::Key::M,
        Key::Character(c) if c == KEY_N => egui::Key::N,
        Key::Character(c) if c == KEY_O => egui::Key::O,
        Key::Character(c) if c == KEY_P => egui::Key::P,
        Key::Character(c) if c == KEY_Q => egui::Key::Q,
        Key::Character(c) if c == KEY_R => egui::Key::R,
        Key::Character(c) if c == KEY_S => egui::Key::S,
        Key::Character(c) if c == KEY_T => egui::Key::T,
        Key::Character(c) if c == KEY_U => egui::Key::U,
        Key::Character(c) if c == KEY_V => egui::Key::V,
        Key::Character(c) if c == KEY_W => egui::Key::W,
        Key::Character(c) if c == KEY_X => egui::Key::X,
        Key::Character(c) if c == KEY_Y => egui::Key::Y,
        Key::Character(c) if c == KEY_Z => egui::Key::Z,
        _ => {
            return None;
        }
    })
}

/// Translates winit to egui modifier keys.
#[inline]
fn winit_to_egui_modifiers(modifiers: ModifiersState) -> egui::Modifiers {
    egui::Modifiers {
        alt: modifiers.alt_key(),
        ctrl: modifiers.control_key(),
        shift: modifiers.shift_key(),
        #[cfg(target_os = "macos")]
        mac_cmd: modifiers.super_key(),
        #[cfg(target_os = "macos")]
        command: modifiers.super_key(),
        #[cfg(not(target_os = "macos"))]
        mac_cmd: false,
        #[cfg(not(target_os = "macos"))]
        command: modifiers.control_key(),
    }
}

#[inline]
fn egui_to_winit_cursor_icon(icon: egui::CursorIcon) -> Option<winit::window::CursorIcon> {
    use egui::CursorIcon::*;

    match icon {
        Default => Some(CursorIcon::Default),
        ContextMenu => Some(CursorIcon::ContextMenu),
        Help => Some(CursorIcon::Help),
        PointingHand => Some(CursorIcon::Pointer),
        Progress => Some(CursorIcon::Progress),
        Wait => Some(CursorIcon::Wait),
        Cell => Some(CursorIcon::Cell),
        Crosshair => Some(CursorIcon::Crosshair),
        Text => Some(CursorIcon::Text),
        VerticalText => Some(CursorIcon::VerticalText),
        Alias => Some(CursorIcon::Alias),
        Copy => Some(CursorIcon::Copy),
        Move => Some(CursorIcon::Move),
        NoDrop => Some(CursorIcon::NoDrop),
        NotAllowed => Some(CursorIcon::NotAllowed),
        Grab => Some(CursorIcon::Grab),
        Grabbing => Some(CursorIcon::Grabbing),
        AllScroll => Some(CursorIcon::AllScroll),
        ResizeHorizontal => Some(CursorIcon::EwResize),
        ResizeNeSw => Some(CursorIcon::NeswResize),
        ResizeNwSe => Some(CursorIcon::NwseResize),
        ResizeVertical => Some(CursorIcon::NsResize),
        ResizeEast => Some(CursorIcon::EResize),
        ResizeSouthEast => Some(CursorIcon::SeResize),
        ResizeSouth => Some(CursorIcon::SResize),
        ResizeSouthWest => Some(CursorIcon::SwResize),
        ResizeWest => Some(CursorIcon::WResize),
        ResizeNorthWest => Some(CursorIcon::NwResize),
        ResizeNorth => Some(CursorIcon::NResize),
        ResizeNorthEast => Some(CursorIcon::NeResize),
        ResizeColumn => Some(CursorIcon::ColResize),
        ResizeRow => Some(CursorIcon::RowResize),
        ZoomIn => Some(CursorIcon::ZoomIn),
        ZoomOut => Some(CursorIcon::ZoomOut),
        None => Option::None,
    }
}
