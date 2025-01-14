// Copyright 2022 Google LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! USB serial interface.

use usb_device::class_prelude::UsbBus;
use usb_device::UsbError;
use usbd_serial::SerialPort;
use wasefire_logger as logger;

use crate::{Error, Unimplemented, Unsupported};

/// USB serial event.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    /// There might be data to read.
    Read,

    /// It might be possible to write data.
    Write,
}

impl From<Event> for crate::Event {
    fn from(event: Event) -> Self {
        super::Event::Serial(event).into()
    }
}

/// USB serial interface.
pub trait Api {
    /// Reads from the USB serial into a buffer.
    ///
    /// Returns the number of bytes read. It could be zero if there's nothing to read.
    fn read(&mut self, output: &mut [u8]) -> Result<usize, Error>;

    /// Writes from a buffer to the USB serial.
    ///
    /// Returns the number of bytes written. It could be zero if the other side is not ready.
    fn write(&mut self, input: &[u8]) -> Result<usize, Error>;

    /// Flushes the USB serial.
    fn flush(&mut self) -> Result<(), Error>;

    /// Enables a given event to be triggered.
    fn enable(&mut self, event: &Event) -> Result<(), Error>;

    /// Disables a given event from being triggered.
    fn disable(&mut self, event: &Event) -> Result<(), Error>;
}

impl Api for Unimplemented {
    fn read(&mut self, _: &mut [u8]) -> Result<usize, Error> {
        unreachable!()
    }

    fn write(&mut self, _: &[u8]) -> Result<usize, Error> {
        unreachable!()
    }

    fn flush(&mut self) -> Result<(), Error> {
        unreachable!()
    }

    fn enable(&mut self, _: &Event) -> Result<(), Error> {
        unreachable!()
    }

    fn disable(&mut self, _: &Event) -> Result<(), Error> {
        unreachable!()
    }
}

impl Api for Unsupported {
    fn read(&mut self, _: &mut [u8]) -> Result<usize, Error> {
        Err(Error::User)
    }

    fn write(&mut self, _: &[u8]) -> Result<usize, Error> {
        Err(Error::User)
    }

    fn flush(&mut self) -> Result<(), Error> {
        Err(Error::User)
    }

    fn enable(&mut self, _: &Event) -> Result<(), Error> {
        Err(Error::User)
    }

    fn disable(&mut self, _: &Event) -> Result<(), Error> {
        Err(Error::User)
    }
}

/// Helper trait for boards using the `usbd_serial` crate.
pub trait HasSerial {
    type UsbBus: UsbBus;

    fn with_serial<R>(&mut self, f: impl FnOnce(&mut Serial<Self::UsbBus>) -> R) -> R;
}

/// Wrapper type for boards using the `usbd_serial` crate.
#[repr(transparent)]
pub struct WithSerial<T: HasSerial>(pub T);

/// Helper struct for boards using the `usbd_serial` crate.
pub struct Serial<'a, T: UsbBus> {
    port: SerialPort<'a, T>,
    read_enabled: bool,
    write_enabled: bool,
}

impl<'a, T: UsbBus> Serial<'a, T> {
    pub fn new(port: SerialPort<'a, T>) -> Self {
        Self { port, read_enabled: false, write_enabled: false }
    }

    pub fn port(&mut self) -> &mut SerialPort<'a, T> {
        &mut self.port
    }

    /// Pushes events based on whether the USB serial was polled.
    pub fn tick(&mut self, polled: bool, mut push: impl FnMut(Event)) {
        if self.read_enabled && polled {
            push(Event::Read);
        }
        if self.write_enabled && self.port.dtr() {
            push(Event::Write);
        }
    }

    fn set(&mut self, event: &Event, enabled: bool) {
        match event {
            Event::Read => self.read_enabled = enabled,
            Event::Write => self.write_enabled = enabled,
        }
    }
}

impl<T: HasSerial> Api for WithSerial<T> {
    fn read(&mut self, output: &mut [u8]) -> Result<usize, Error> {
        match self.0.with_serial(|serial| serial.port.read(output)) {
            Ok(len) => {
                logger::trace!("{}{:?} = read({})", len, &output[.. len], output.len());
                Ok(len)
            }
            Err(UsbError::WouldBlock) => Ok(0),
            Err(e) => {
                logger::debug!("{} = read({})", logger::Debug2Format(&e), output.len());
                Err(Error::World)
            }
        }
    }

    fn write(&mut self, input: &[u8]) -> Result<usize, Error> {
        if !self.0.with_serial(|serial| serial.port.dtr()) {
            // Data terminal is not ready.
            return Ok(0);
        }
        match self.0.with_serial(|serial| serial.port.write(input)) {
            Ok(len) => {
                logger::trace!("{} = write({}{:?})", len, input.len(), input);
                Ok(len)
            }
            Err(UsbError::WouldBlock) => Ok(0),
            Err(e) => {
                logger::debug!("{} = write({}{:?})", logger::Debug2Format(&e), input.len(), input);
                Err(Error::World)
            }
        }
    }

    fn flush(&mut self) -> Result<(), Error> {
        match self.0.with_serial(|serial| serial.port.flush()) {
            Ok(()) => {
                logger::trace!("flush()");
                Ok(())
            }
            Err(e) => {
                logger::debug!("{} = flush()", logger::Debug2Format(&e));
                Err(Error::World)
            }
        }
    }

    fn enable(&mut self, event: &Event) -> Result<(), Error> {
        self.0.with_serial(|serial| serial.set(event, true));
        Ok(())
    }

    fn disable(&mut self, event: &Event) -> Result<(), Error> {
        self.0.with_serial(|serial| serial.set(event, false));
        Ok(())
    }
}
