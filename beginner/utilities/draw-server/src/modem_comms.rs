use serialport::prelude::*;
use postcard::{from_bytes, to_slice_cobs};
use nrf52_bin_logger::LogOnLine;
use protocol::{ModemUartMessages, CellCommand};
use std::sync::mpsc::Sender;
use std::time::{Instant, Duration};

struct Modem {
    port: Box<dyn SerialPort>,
    cobs_buf: Vec<u8>,
    since_last_err: usize,
}

impl Modem {
    fn process_serial(&mut self) -> Result<Vec<CellCommand>, ()> {
        let mut buf = [0u8; 1024];
        let buf = match self.port.read(&mut buf) {
            Ok(ct) => &buf[..ct],
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => return Ok(vec![]),
            Err(e) => {
                eprintln!("{:?}", e);
                return Err(());
            }
        };

        self.push_bytes(buf)
    }

    fn push_bytes(&mut self, mut data: &[u8]) -> Result<Vec<CellCommand>, ()> {
        let mut resps = vec![];

        while let Some(idx) = data.iter().position(|&b| b == 0) {
            let (end, rest) = data.split_at(idx+1);
            self.cobs_buf.extend_from_slice(end);

            use LogOnLine::*;
            use ModemUartMessages::*;
            if let Ok(idx) = cobs::decode_in_place(&mut self.cobs_buf) {
                match from_bytes::<LogOnLine<ModemUartMessages>>(&self.cobs_buf[..idx]) {
                    Ok(ProtocolMessage(SetCell(desmsg))) =>  {
                        self.since_last_err += 1;
                        resps.push(desmsg);
                    }
                    Ok(ProtocolMessage(Loopback(val))) =>  {
                        self.since_last_err += 1;
                        eprintln!("Got Loopback! Good: {}", val == 0x4242_4242);
                    }
                    Ok(other) => {
                        self.since_last_err += 1;
                        display(&other)
                    },
                    Err(e) => {
                        eprintln!("bad_decode: {:?}, since_last: {}", e, self.since_last_err);
                        self.since_last_err = 0;
                    }
                }
            } else {
                eprintln!("Bad Cobs, since_last: {}", self.since_last_err);
                self.since_last_err = 0;
            }

            data = rest;
            self.cobs_buf.clear();
        }

        self.cobs_buf.extend_from_slice(data);

        Ok(resps)
    }
}

pub fn modem_task(port: Box<dyn SerialPort>, prod_cmds: Sender<CellCommand>) -> Result<(), ()> {
    println!("Receiving data on {} at {} baud:", port.name().unwrap(), port.baud_rate().unwrap());

    let mut modem = Modem {
        port,
        cobs_buf: vec![],
        since_last_err: 0,
    };

    let mut last = Instant::now();

    loop {
        if last.elapsed() >= Duration::from_millis(500) {
            let mut buf = [0u8; 1024];
            let buf2 = to_slice_cobs(
                &ModemUartMessages::Loopback(0x4242_4242),
                &mut buf
            ).unwrap();

            modem.port.write(&buf2).unwrap();

            last = Instant::now();
        }
        modem.process_serial()?
            .drain(..)
            .try_for_each(|m| {
                prod_cmds.send(m).map_err(|_| ())
            })?;
    }
}

fn display(msg: &LogOnLine<ModemUartMessages>) {
    match msg {
        LogOnLine::Log(log) => {
            eprintln!("{}", prefixed_lines(log, "LOG"));
        }
        LogOnLine::Warn(log) => {
            eprintln!("{}", prefixed_lines(log, "WRN"));
        }
        LogOnLine::Error(log) => {
            eprintln!("{}", prefixed_lines(log, "ERR"));
        }
        LogOnLine::BinaryRaw(log) => {
            eprintln!("{}", prefixed_lines(&format!("{:02X?}", log), "BIN"));
        }
        _ => {}
    }
}

use chrono::prelude::*;

fn prefixed_lines(st: &str, msg: &str) -> String {
    let mut out = String::new();
    out += &format!("{:?}\n", Local::now());
    st.lines().for_each(|line| {
        out += &format!(
            " => {}: {}\n",
            msg,
            line
        );
    });
    out.truncate(out.trim_end().len());
    out
}
