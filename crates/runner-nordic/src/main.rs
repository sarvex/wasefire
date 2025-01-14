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

#![no_std]
#![no_main]
#![feature(try_blocks)]

extern crate alloc;

mod allocator;
mod storage;
#[cfg(feature = "debug")]
mod systick;
mod tasks;

use core::cell::{Cell, RefCell};
use core::mem::MaybeUninit;
use core::ops::DerefMut;

use cortex_m::peripheral::NVIC;
use cortex_m_rt::entry;
use critical_section::Mutex;
#[cfg(feature = "debug")]
use defmt_rtt as _;
use nrf52840_hal::ccm::{Ccm, DataRate};
use nrf52840_hal::clocks::{self, ExternalOscillator, Internal, LfOscStopped};
use nrf52840_hal::gpio;
use nrf52840_hal::gpio::{Level, Output, Pin, PushPull};
use nrf52840_hal::gpiote::Gpiote;
use nrf52840_hal::pac::{interrupt, Interrupt};
use nrf52840_hal::prelude::InputPin;
use nrf52840_hal::rng::Rng;
use nrf52840_hal::usbd::{UsbPeripheral, Usbd};
#[cfg(feature = "release")]
use panic_abort as _;
#[cfg(feature = "debug")]
use panic_probe as _;
use storage::Storage;
use tasks::button::{channel, Button};
use tasks::clock::Timers;
use tasks::usb::Usb;
use tasks::Events;
use usb_device::class_prelude::UsbBusAllocator;
use usb_device::device::{UsbDevice, UsbDeviceBuilder, UsbVidPid};
use usbd_serial::{SerialPort, USB_CLASS_CDC};
use wasefire_board_api::usb::serial::Serial;
use wasefire_scheduler::Scheduler;
use {wasefire_board_api as board, wasefire_logger as logger};

#[cfg(feature = "debug")]
#[defmt::panic_handler]
fn panic() -> ! {
    panic_probe::hard_fault();
}

type Clocks = clocks::Clocks<ExternalOscillator, Internal, LfOscStopped>;

struct State {
    events: Events,
    buttons: [Button; 4],
    gpiote: Gpiote,
    serial: Serial<'static, Usb>,
    timers: Timers,
    ccm: Ccm,
    leds: [Pin<Output<PushPull>>; 4],
    rng: Rng,
    storage: Option<Storage>,
    usb_dev: UsbDevice<'static, Usb>,
}

#[derive(Copy, Clone)]
struct Board(&'static Mutex<RefCell<State>>);

static BOARD: Mutex<Cell<Option<Board>>> = Mutex::new(Cell::new(None));

fn get_board() -> Board {
    critical_section::with(|cs| BOARD.borrow(cs).get()).unwrap()
}

#[entry]
fn main() -> ! {
    static mut CLOCKS: MaybeUninit<Clocks> = MaybeUninit::uninit();
    static mut USB_BUS: MaybeUninit<UsbBusAllocator<Usb>> = MaybeUninit::uninit();
    static mut STATE: MaybeUninit<Mutex<RefCell<State>>> = MaybeUninit::uninit();

    #[cfg(feature = "debug")]
    let c = nrf52840_hal::pac::CorePeripherals::take().unwrap();
    #[cfg(feature = "debug")]
    systick::init(c.SYST);
    allocator::init();
    logger::debug!("Runner starts.");
    let p = nrf52840_hal::pac::Peripherals::take().unwrap();
    let port0 = gpio::p0::Parts::new(p.P0);
    let buttons = [
        Button::new(port0.p0_11.into_pullup_input().degrade()),
        Button::new(port0.p0_12.into_pullup_input().degrade()),
        Button::new(port0.p0_24.into_pullup_input().degrade()),
        Button::new(port0.p0_25.into_pullup_input().degrade()),
    ];
    let leds = [
        port0.p0_13.into_push_pull_output(Level::High).degrade(),
        port0.p0_14.into_push_pull_output(Level::High).degrade(),
        port0.p0_15.into_push_pull_output(Level::High).degrade(),
        port0.p0_16.into_push_pull_output(Level::High).degrade(),
    ];
    let timers = Timers::new(p.TIMER0, p.TIMER1, p.TIMER2, p.TIMER3, p.TIMER4);
    let gpiote = Gpiote::new(p.GPIOTE);
    // We enable all USB interrupts except STARTED and EPDATA which are feedback loops.
    p.USBD.inten.write(|w| unsafe { w.bits(0x00fffffd) });
    let clocks = CLOCKS.write(clocks::Clocks::new(p.CLOCK).enable_ext_hfosc());
    let usb_bus = UsbBusAllocator::new(Usbd::new(UsbPeripheral::new(p.USBD, clocks)));
    let usb_bus = USB_BUS.write(usb_bus);
    let serial = Serial::new(SerialPort::new(usb_bus));
    let usb_dev = UsbDeviceBuilder::new(usb_bus, UsbVidPid(0x16c0, 0x27dd))
        .product("Serial port")
        .device_class(USB_CLASS_CDC)
        .build();
    let rng = Rng::new(p.RNG);
    let ccm = Ccm::init(p.CCM, p.AAR, DataRate::_1Mbit);
    let storage = Some(Storage::new(p.NVMC));
    let events = Events::default();
    let state = STATE.write(Mutex::new(RefCell::new(State {
        events,
        buttons,
        gpiote,
        serial,
        timers,
        ccm,
        leds,
        rng,
        storage,
        usb_dev,
    })));
    // We first set the board and then enable interrupts so that interrupts may assume the board is
    // always present.
    critical_section::with(|cs| BOARD.borrow(cs).set(Some(Board(state))));
    for &interrupt in INTERRUPTS {
        unsafe { NVIC::unmask(interrupt) };
    }
    logger::debug!("Runner is initialized.");
    const WASM: &[u8] = include_bytes!("../../../target/applet.wasm");
    Scheduler::run(Board(state), WASM)
}

macro_rules! interrupts {
    ($($name:ident = $func:ident$(($($arg:expr),*$(,)?))?),*$(,)?) => {
        const INTERRUPTS: &[Interrupt] = &[$(Interrupt::$name),*];
        $(
            #[interrupt]
            fn $name() {
                $func(get_board()$($(, $arg)*)?);
            }
        )*
    };
}

interrupts! {
    GPIOTE = gpiote,
    TIMER0 = timer(0),
    TIMER1 = timer(1),
    TIMER2 = timer(2),
    TIMER3 = timer(3),
    TIMER4 = timer(4),
    USBD = usbd,
}

fn gpiote(board: Board) {
    critical_section::with(|cs| {
        let mut state = board.0.borrow_ref_mut(cs);
        let state = state.deref_mut();
        for (i, button) in state.buttons.iter_mut().enumerate() {
            if channel(&state.gpiote, i).is_event_triggered() {
                let pressed = button.pin.is_low().unwrap();
                state.events.push(board::button::Event { button: i, pressed }.into());
            }
        }
        state.gpiote.reset_events();
    });
}

fn timer(board: Board, timer: usize) {
    critical_section::with(|cs| {
        let mut state = board.0.borrow_ref_mut(cs);
        state.events.push(board::timer::Event { timer }.into());
        state.timers.tick(timer);
    })
}

fn usbd(board: Board) {
    #[cfg(feature = "debug")]
    {
        use core::sync::atomic::AtomicU32;
        use core::sync::atomic::Ordering::SeqCst;
        static COUNT: AtomicU32 = AtomicU32::new(0);
        static MASK: AtomicU32 = AtomicU32::new(0);
        let count = COUNT.fetch_add(1, SeqCst).wrapping_add(1);
        let mut mask = 0;
        for i in 0 ..= 24 {
            let x = (0x40027100 + 4 * i) as *const u32;
            let x = unsafe { core::ptr::read_volatile(x) };
            core::assert!(x <= 1);
            mask |= x << i;
        }
        mask |= MASK.fetch_or(mask, SeqCst);
        if count % 1000 == 0 {
            logger::trace!("Got {} USB interrupts matching {:08x}.", count, mask);
            MASK.store(0, SeqCst);
        }
    }
    critical_section::with(|cs| {
        let mut state = board.0.borrow_ref_mut(cs);
        let state = state.deref_mut();
        let polled = state.usb_dev.poll(&mut [state.serial.port()]);
        state.serial.tick(polled, |event| state.events.push(event.into()));
    });
}
