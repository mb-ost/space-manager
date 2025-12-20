use anyhow::Result;
use evdev::{Device, EventType, InputEventKind, Key};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info};

#[derive(Debug, Clone)]
pub enum MouseButton {
    Button4, // Back button (275)
    Button5, // Forward button (276)
}

pub struct InputListener {
    sender: mpsc::UnboundedSender<MouseButton>,
}

impl InputListener {
    pub fn new() -> (Self, mpsc::UnboundedReceiver<MouseButton>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self { sender }, receiver)
    }

    /// Start listening to mouse input events in a separate thread
    pub fn start(self: Arc<Self>) -> Result<()> {
        std::thread::spawn(move || {
            if let Err(e) = self.listen_loop() {
                error!("Input listener error: {}", e);
            }
        });

        Ok(())
    }

    fn listen_loop(&self) -> Result<()> {
        // Enumerate all devices and log them
        info!("Enumerating input devices...");
        let all_devices: Vec<_> = evdev::enumerate().collect();
        info!("Found {} total input devices", all_devices.len());

        // Find mouse devices - look for any device with mouse buttons
        let devices: Vec<_> = all_devices
            .into_iter()
            .filter_map(|(path, device)| {
                let name = device.name().unwrap_or("unknown");
                let has_keys = device.supported_keys().is_some();

                if !has_keys {
                    debug!("Device {} at {:?} has no key support", name, path);
                    return None;
                }

                let keys = device.supported_keys().unwrap();
                let has_mouse_buttons = keys.contains(Key::BTN_LEFT) ||
                    keys.contains(Key::BTN_RIGHT) ||
                    keys.contains(Key::BTN_SIDE) ||
                    keys.contains(Key::BTN_EXTRA);

                if has_mouse_buttons {
                    info!("✓ Found mouse device: {} at {:?}", name, path);
                    Some(device)
                } else {
                    debug!("✗ Device {} at {:?} has no mouse buttons", name, path);
                    None
                }
            })
            .collect();

        if devices.is_empty() {
            error!("No mouse devices found. You may need to run with sudo or add your user to the 'input' group.");
            error!("Try: sudo usermod -aG input $USER && newgrp input");
            return Ok(());
        }

        info!("Monitoring {} input device(s) for side buttons", devices.len());

        // Monitor all mouse devices
        for device in devices {
            let sender = self.sender.clone();
            std::thread::spawn(move || {
                if let Err(e) = Self::monitor_device(device, sender) {
                    error!("Device monitoring error: {}", e);
                }
            });
        }

        // Keep thread alive
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }

    fn monitor_device(
        mut device: Device,
        sender: mpsc::UnboundedSender<MouseButton>,
    ) -> Result<()> {
        info!("Monitoring device: {}", device.name().unwrap_or("unknown"));

        loop {
            for event in device.fetch_events()? {
                if event.event_type() == EventType::KEY {
                    // Only process button press (value 1), not release (value 0)
                    if event.value() == 1 {
                        match event.kind() {
                            InputEventKind::Key(Key::BTN_SIDE) => {
                                debug!("Mouse button 4 (back) pressed");
                                let _ = sender.send(MouseButton::Button4);
                            }
                            InputEventKind::Key(Key::BTN_EXTRA) => {
                                debug!("Mouse button 5 (forward) pressed");
                                let _ = sender.send(MouseButton::Button5);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
}
