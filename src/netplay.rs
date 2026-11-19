use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    time::Duration,
};

use kawaks_rs::{FrameInput, FrameReport};

const VERSION: &[u8; 4] = b"KWN2";
const BUTTONS: u16 = 0x33ff;

pub struct Peer {
    stream: TcpStream,
    player: usize,
}

impl Peer {
    pub fn listen(address: SocketAddr, rom: [u32; 4], state: u32) -> io::Result<Self> {
        let listener = TcpListener::bind(address)?;
        eprintln!("Waiting for player 2 at {}", listener.local_addr()?);
        Self::start(listener.accept()?.0, 0, rom, state)
    }

    #[cfg(feature = "sdl2")]
    pub fn listen_until(
        address: SocketAddr,
        rom: [u32; 4],
        state: u32,
        stop: &std::sync::atomic::AtomicBool,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind(address)?;
        listener.set_nonblocking(true)?;
        while !stop.load(std::sync::atomic::Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false)?;
                    return Self::start(stream, 0, rom, state);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(30));
                }
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "LAN hosting cancelled",
        ))
    }

    pub fn connect(address: SocketAddr, rom: [u32; 4], state: u32) -> io::Result<Self> {
        Self::start(
            TcpStream::connect_timeout(&address, Duration::from_secs(10))?,
            1,
            rom,
            state,
        )
    }

    fn start(mut stream: TcpStream, player: usize, rom: [u32; 4], state: u32) -> io::Result<Self> {
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;

        let mut local = [0; 24];
        local[..4].copy_from_slice(VERSION);
        for (bytes, value) in local[4..]
            .chunks_exact_mut(4)
            .zip(rom.into_iter().chain([state]))
        {
            bytes.copy_from_slice(&value.to_be_bytes());
        }
        stream.write_all(&local)?;
        let mut remote = [0; 24];
        stream.read_exact(&mut remote)?;
        if local != remote {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "netplay requires the same protocol, ROM and initial state on both peers",
            ));
        }
        Ok(Self { stream, player })
    }

    pub fn exchange(&mut self, buttons: u16, before: FrameReport) -> io::Result<FrameInput> {
        let mut packet = [0; 14];
        packet[..8].copy_from_slice(&before.frame.to_be_bytes());
        packet[8..12].copy_from_slice(&before.state_crc32.to_be_bytes());
        packet[12..].copy_from_slice(&(buttons & BUTTONS).to_be_bytes());
        self.stream.write_all(&packet)?;
        let mut remote = [0; 14];
        self.stream.read_exact(&mut remote)?;
        if remote[..12] != packet[..12] {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("netplay desynchronized before frame {}", before.frame + 1),
            ));
        }
        let other = u16::from_be_bytes([remote[12], remote[13]]);
        if other & !BUTTONS != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid peer input",
            ));
        }
        let mut input = FrameInput::default();
        input.players[self.player] = buttons & BUTTONS;
        input.players[1 - self.player] = other;
        Ok(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_peers_share_frame_inputs_and_detect_state_mismatch() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let state = FrameReport {
            frame: 1,
            state_crc32: 42,
            main_cycles: 0,
            sound_cycles: 0,
            main_pc: 0,
            sound_pc: 0,
            work_crc32: 0,
            graphics_ram_crc32: 0,
            qsound_commands: 0,
        };
        std::thread::scope(|scope| {
            let thread = scope.spawn(|| {
                let mut peer = Peer::start(listener.accept().unwrap().0, 0, [1; 4], 42).unwrap();
                assert_eq!(
                    peer.exchange(0x320, state).unwrap().players,
                    [0x320, 0x1000, 0, 0]
                );
                assert_eq!(
                    peer.exchange(0, state).unwrap_err().kind(),
                    io::ErrorKind::InvalidData
                );
            });
            let mut peer = Peer::connect(address, [1; 4], 42).unwrap();
            assert_eq!(
                peer.exchange(0x1000, state).unwrap().players,
                [0x320, 0x1000, 0, 0]
            );
            assert_eq!(
                peer.exchange(
                    0,
                    FrameReport {
                        state_crc32: 43,
                        ..state
                    }
                )
                .unwrap_err()
                .kind(),
                io::ErrorKind::InvalidData
            );
            thread.join().unwrap();
        });
    }
}
