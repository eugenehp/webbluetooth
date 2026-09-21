//! Open an L2CAP channel to a device and stream bytes over it.
//!
//! ```sh
//! cargo run -p webbluetooth --example l2cap -- 0x0080
//! ```
//!
//! Not part of Web Bluetooth. An L2CAP connection-oriented channel is the one
//! place Bluetooth stops being request/response: a bidirectional byte pipe
//! with no 512-byte attribute ceiling and no ATT round trip per write, which
//! is what you want for a firmware image or a stream.
//!
//! **The peer has to be listening.** A PSM is not something a device offers by
//! default — the far side must have published one, and told you which. There
//! is no discovery for it, so the number is an argument here. `0x0080` is the
//! first dynamic PSM and is what this crate's own `peripheral` example
//! publishes, so the two can be pointed at each other.
//!
//! Absent on Windows, which has no LE L2CAP channel API in WinRT or Win32;
//! [`webbluetooth::L2CAP`] is the constant to branch on.

#[cfg(not(l2cap))]
fn main() {
    eprintln!("This platform has no L2CAP channel API — see webbluetooth::L2CAP.");
    std::process::exit(1);
}

#[cfg(l2cap)]
fn main() -> webbluetooth::Result<()> {
    futures_executor::block_on(run())
}

#[cfg(l2cap)]
async fn run() -> webbluetooth::Result<()> {
    use webbluetooth::chooser::StrongestSignal;
    use webbluetooth::prelude::*;

    // `0x0080` is the first PSM in the dynamic range.
    let psm = match std::env::args().nth(1) {
        Some(arg) => {
            let text = arg.trim_start_matches("0x");
            u16::from_str_radix(text, if arg.starts_with("0x") { 16 } else { 10 })
                .unwrap_or_else(|_| panic!("not a PSM: {arg}"))
        }
        None => 0x0080,
    };

    let bluetooth = Bluetooth::with_chooser(StrongestSignal::default());
    if let Err(why) = bluetooth.availability().await {
        eprintln!("Bluetooth unavailable: {why} — try the `doctor` example.");
        std::process::exit(1);
    }

    println!("looking for something to talk to…");
    let device = bluetooth
        .request_device(RequestDeviceOptions::new().accept_all_devices())
        .await?;
    println!("{}", device.name().unwrap_or_else(|| device.id().into()));

    // The link has to be up first: a channel is opened over an existing
    // connection, not instead of one.
    //
    // Bounded, because this example accepts any device: `connect()` waits for
    // the peripheral as long as it takes, and plenty of what a wide scan finds
    // advertises happily while accepting no connections at all. Without a
    // deadline that is an example that sits in silence.
    let gatt = device.gatt();
    match webbluetooth::timeout(std::time::Duration::from_secs(15), gatt.connect()).await {
        Ok(result) => result?,
        Err(()) => {
            eprintln!("connect timed out after 15 s — that device is not accepting connections.");
            std::process::exit(1);
        }
    }

    let channel = device.open_l2cap_channel(psm).await?;
    println!("  channel   open on PSM {:#06x}", channel.psm());

    // There is one inbound stream and taking it twice would split the bytes
    // between two consumers, so `take_incoming` hands it over exactly once.
    let mut incoming = channel.take_incoming().expect("the first and only call");

    channel.send(b"hello")?;
    // `send` returns once queued, not once on the air.
    channel.flushed().await?;
    println!("  sent      5 bytes");

    // A chunk is whatever one read returned — L2CAP CoC is a byte stream, so
    // message boundaries are yours to impose.
    let mut total = 0usize;
    while let Some(chunk) = incoming.next().await {
        total += chunk.len();
        println!("  received  {} bytes ({total} in total)", chunk.len());
    }

    // The stream ending means the channel closed; `on_close` says why.
    println!("  closed    {:?}", channel.on_close().await);
    Ok(())
}
