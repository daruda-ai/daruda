use std::io::{self, Write};
use std::sync::mpsc;

#[derive(Clone)]
/// A PTY input queue drained by the session's writer thread.
pub struct PtyInput {
    sender: Option<mpsc::Sender<InputWrite>>,
}

pub(super) struct InputWrite {
    bytes: Vec<u8>,
}

impl PtyInput {
    pub(super) fn channel() -> (Self, mpsc::Receiver<InputWrite>) {
        let (sender, receiver) = mpsc::channel();
        (
            Self {
                sender: Some(sender),
            },
            receiver,
        )
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn stub() -> Self {
        Self { sender: None }
    }

    /// Enqueue keyboard input without waiting for its write.
    pub fn send(&self, bytes: Vec<u8>) -> Result<(), mpsc::SendError<Vec<u8>>> {
        match &self.sender {
            Some(sender) => sender
                .send(InputWrite { bytes })
                .map_err(|error| mpsc::SendError(error.0.bytes)),
            None => Ok(()),
        }
    }
}

impl InputWrite {
    pub(super) fn write_to(self, writer: &mut impl Write) -> io::Result<()> {
        writer.write_all(&self.bytes).and_then(|()| writer.flush())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sent_input_reaches_the_writer() {
        let (input, pending) = PtyInput::channel();
        input.send(b"keys".to_vec()).unwrap();
        let mut output = Vec::new();
        pending.recv().unwrap().write_to(&mut output).unwrap();
        assert_eq!(output, b"keys");
    }

    #[test]
    fn a_closed_writer_hands_the_input_back() {
        let (input, pending) = PtyInput::channel();
        drop(pending);
        assert_eq!(input.send(b"keys".to_vec()).unwrap_err().0, b"keys");
        assert!(PtyInput::stub().send(b"keys".to_vec()).is_ok());
    }

    struct RefusingWriter;

    impl Write for RefusingWriter {
        fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_kernel_write_failure_is_returned_to_the_writer_thread() {
        let (input, pending) = PtyInput::channel();
        input.send(b"keys".to_vec()).unwrap();
        assert!(
            pending
                .recv()
                .unwrap()
                .write_to(&mut RefusingWriter)
                .is_err()
        );
    }
}
