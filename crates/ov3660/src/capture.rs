// Modified from the attributed upstream implementation for this crate.
use embassy_time::{Duration, Timer};
use esp_hal::dma::{DmaError, DmaRxStreamBuf};

use crate::driver::{AsyncCameraDriver, AsyncCameraTransfer};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CaptureConfig {
    pub dma_wait_timeout_ms: u64,
    pub dma_recovery_delay_ms: u64,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            dma_wait_timeout_ms: 300,
            dma_recovery_delay_ms: 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureEvent {
    None,
    Timeout,
    DmaError,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChunkResult {
    pub len: usize,
    pub eof: bool,
    pub resyncing: bool,
    pub event: CaptureEvent,
}

enum CaptureState<'d> {
    Idle {
        driver: AsyncCameraDriver<'d>,
        buffer: DmaRxStreamBuf,
    },
    Active(AsyncCameraTransfer<'d, DmaRxStreamBuf>),
    Transitioning,
}

pub struct CameraCapture<'d> {
    state: CaptureState<'d>,
    config: CaptureConfig,
    resyncing: bool,
}

impl<'d> CameraCapture<'d> {
    pub const fn new(
        driver: AsyncCameraDriver<'d>,
        buffer: DmaRxStreamBuf,
        config: CaptureConfig,
    ) -> Self {
        Self {
            state: CaptureState::Idle { driver, buffer },
            config,
            resyncing: false,
        }
    }

    pub fn start(&mut self) -> Result<(), DmaError> {
        let CaptureState::Idle { .. } = self.state else {
            return Ok(());
        };
        let CaptureState::Idle { driver, buffer } =
            core::mem::replace(&mut self.state, CaptureState::Transitioning)
        else {
            unreachable!()
        };
        match driver.receive(buffer) {
            Ok(transfer) => {
                self.state = CaptureState::Active(transfer);
                Ok(())
            }
            Err((error, driver, buffer)) => {
                self.state = CaptureState::Idle { driver, buffer };
                Err(error)
            }
        }
    }

    pub fn stop_capture(&mut self) {
        match core::mem::replace(&mut self.state, CaptureState::Transitioning) {
            CaptureState::Active(transfer) => {
                let (driver, buffer) = transfer.stop();
                self.state = CaptureState::Idle { driver, buffer };
            }
            state => self.state = state,
        }
        self.resyncing = false;
    }

    pub const fn is_resyncing(&self) -> bool {
        self.resyncing
    }

    pub async fn next_chunk(&mut self, output: &mut [u8]) -> ChunkResult {
        if matches!(self.state, CaptureState::Idle { .. }) && self.start().is_err() {
            self.resyncing = true;
            return self.empty(CaptureEvent::DmaError);
        }

        let wait_result = {
            let CaptureState::Active(transfer) = &mut self.state else {
                unreachable!()
            };
            embassy_time::with_timeout(
                Duration::from_millis(self.config.dma_wait_timeout_ms),
                transfer.wait(),
            )
            .await
        };
        match wait_result {
            Ok(Ok(())) => {}
            Ok(Err(_)) => return self.recover(CaptureEvent::DmaError).await,
            Err(_) => return self.recover(CaptureEvent::Timeout).await,
        }

        if matches!(&self.state, CaptureState::Active(transfer) if transfer.is_descriptor_empty()) {
            return self.recover(CaptureEvent::DmaError).await;
        }

        let (copied, eof) = {
            let CaptureState::Active(transfer) = &mut self.state else {
                unreachable!()
            };
            let (available_len, copied, source_eof) = {
                let (available, source_eof) = transfer.peek_until_eof();
                let copied = available.len().min(output.len());
                output[..copied].copy_from_slice(&available[..copied]);
                (available.len(), copied, source_eof)
            };
            transfer.consume(copied);
            (copied, source_eof && copied == available_len)
        };

        let CaptureState::Active(transfer) = &mut self.state else {
            unreachable!()
        };
        transfer.resume_dma();

        if eof {
            self.resyncing = false;
        }
        ChunkResult {
            len: copied,
            eof,
            resyncing: self.resyncing,
            event: CaptureEvent::None,
        }
    }

    async fn recover(&mut self, event: CaptureEvent) -> ChunkResult {
        self.resyncing = true;
        let CaptureState::Active(transfer) =
            core::mem::replace(&mut self.state, CaptureState::Transitioning)
        else {
            return self.empty(event);
        };
        let (driver, buffer) = transfer.stop();
        Timer::after(Duration::from_millis(self.config.dma_recovery_delay_ms)).await;
        match driver.receive(buffer) {
            Ok(transfer) => self.state = CaptureState::Active(transfer),
            Err((_, driver, buffer)) => self.state = CaptureState::Idle { driver, buffer },
        }
        self.empty(event)
    }

    const fn empty(&self, event: CaptureEvent) -> ChunkResult {
        ChunkResult {
            len: 0,
            eof: false,
            resyncing: true,
            event,
        }
    }
}
