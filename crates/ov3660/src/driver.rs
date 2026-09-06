// Modified from the attributed upstream implementations for this crate.
use core::{
    future::poll_fn,
    mem::ManuallyDrop,
    ops::{Deref, DerefMut},
    sync::atomic::{Ordering, compiler_fence},
    task::{Context, Poll},
};

use esp_hal::{
    dma::{
        AhbGdmaChannel, DmaChannel, DmaError, DmaRxBuffer, DmaRxInterrupt, InterruptAccess,
        RegisterAccess, RxRegisterAccess,
    },
    gpio::{
        InputConfig, InputSignal, OutputConfig, OutputSignal,
        interconnect::{PeripheralInput, PeripheralOutput},
    },
    lcd_cam::{
        BitOrder, ByteOrder, CamDmaRxChannel, ClockError,
        cam::{Config, ConfigError, EofMode, VhdeMode},
    },
    peripherals::LCD_CAM,
};

type CameraDmaRx<'d> = <AhbGdmaChannel<'d> as DmaChannel>::Rx;
// ESP32-S3 GDMA peripheral selector value assigned to LCD_CAM.
const LCD_CAM_DMA_ID: u8 = 5;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ConverterMode {
    #[default]
    Bypass,
    Yuv422Passthrough,
}

pub struct AsyncCameraDriver<'d> {
    lcd_cam: LCD_CAM<'d>,
    dma: CameraDmaRx<'d>,
    initial_dma_addr: u32,
}

impl<'d> AsyncCameraDriver<'d> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        lcd_cam: LCD_CAM<'d>,
        dma: impl CamDmaRxChannel<'d>,
        config: Config,
        converter: ConverterMode,
        m_clk: impl PeripheralOutput<'d>,
        p_clk: impl PeripheralInput<'d>,
        v_sync_pin: impl PeripheralInput<'d>,
        h_ref_pin: impl PeripheralInput<'d>,
        data_0_pin: impl PeripheralInput<'d>,
        data_1_pin: impl PeripheralInput<'d>,
        data_2_pin: impl PeripheralInput<'d>,
        data_3_pin: impl PeripheralInput<'d>,
        data_4_pin: impl PeripheralInput<'d>,
        data_5_pin: impl PeripheralInput<'d>,
        data_6_pin: impl PeripheralInput<'d>,
        data_7_pin: impl PeripheralInput<'d>,
    ) -> Result<Self, ConfigError> {
        esp_hal::peripherals::SYSTEM::regs()
            .perip_clk_en1()
            .modify(|_, w| w.lcd_cam_clk_en().set_bit().dma_clk_en().set_bit());
        esp_hal::peripherals::SYSTEM::regs()
            .perip_rst_en1()
            .modify(|_, w| w.lcd_cam_rst().set_bit().dma_rst().set_bit());
        esp_hal::peripherals::SYSTEM::regs()
            .perip_rst_en1()
            .modify(|_, w| w.lcd_cam_rst().clear_bit().dma_rst().clear_bit());

        let m_clk = m_clk.into();
        m_clk.apply_output_config(&OutputConfig::default());
        m_clk.set_output_enable(true);
        OutputSignal::CAM_CLK.connect_to(&m_clk);
        Self::connect_input(InputSignal::CAM_PCLK, p_clk);
        Self::connect_input(InputSignal::CAM_V_SYNC, v_sync_pin);
        Self::connect_input(InputSignal::CAM_H_ENABLE, h_ref_pin);
        Self::connect_input(InputSignal::CAM_DATA_0, data_0_pin);
        Self::connect_input(InputSignal::CAM_DATA_1, data_1_pin);
        Self::connect_input(InputSignal::CAM_DATA_2, data_2_pin);
        Self::connect_input(InputSignal::CAM_DATA_3, data_3_pin);
        Self::connect_input(InputSignal::CAM_DATA_4, data_4_pin);
        Self::connect_input(InputSignal::CAM_DATA_5, data_5_pin);
        Self::connect_input(InputSignal::CAM_DATA_6, data_6_pin);
        Self::connect_input(InputSignal::CAM_DATA_7, data_7_pin);

        let xclk_hz = config.frequency().as_hz() as u32;
        let divider = 160_000_000u32
            .checked_div(xclk_hz)
            .filter(|value| (2..=256).contains(value))
            .ok_or(ConfigError::Clock(ClockError::FrequencyTooLow))? as u8;
        if config.line_interrupt().is_some_and(|value| value > 0x7f) {
            return Err(ConfigError::LineInterrupt);
        }

        lcd_cam.register_block().cam_ctrl().write(|w| unsafe {
            w.cam_clk_sel().bits(3);
            w.cam_clkm_div_num().bits(divider);
            w.cam_clkm_div_a().bits(0);
            w.cam_clkm_div_b().bits(0);
            if let Some(threshold) = config.vsync_filter_threshold() {
                w.cam_vsync_filter_thres().bits(threshold as _);
            }
            w.cam_byte_order()
                .bit(config.byte_order() != ByteOrder::default());
            w.cam_bit_order()
                .bit(config.bit_order() != BitOrder::default());
            w.cam_vs_eof_en()
                .bit(matches!(config.eof_mode(), EofMode::VsyncSignal));
            w.cam_line_int_en().bit(config.line_interrupt().is_some());
            w.cam_stop_en().set_bit()
        });
        lcd_cam.register_block().cam_ctrl1().write(|w| unsafe {
            w.cam_2byte_en().bit(config.enable_2byte_mode());
            w.cam_vh_de_mode_en()
                .bit(matches!(config.vh_de_mode(), VhdeMode::VsyncHsync));
            if let EofMode::ByteLen(length) = config.eof_mode() {
                w.cam_rec_data_bytelen().bits(length);
            }
            if let Some(line) = config.line_interrupt() {
                w.cam_line_int_num().bits(line);
            }
            w.cam_vsync_filter_en()
                .bit(config.vsync_filter_threshold().is_some());
            w.cam_clk_inv().bit(config.invert_pixel_clock());
            w.cam_de_inv().bit(config.invert_h_enable());
            w.cam_hsync_inv().bit(config.invert_hsync());
            w.cam_vsync_inv().bit(config.invert_vsync())
        });
        lcd_cam
            .register_block()
            .cam_rgb_yuv()
            .write(|w| match converter {
                ConverterMode::Bypass => w.cam_conv_bypass().clear_bit(),
                ConverterMode::Yuv422Passthrough => unsafe {
                    w.cam_conv_bypass().set_bit();
                    w.cam_conv_mode_8bits_on().set_bit();
                    w.cam_conv_trans_mode().set_bit();
                    w.cam_conv_yuv2yuv_mode().bits(0);
                    w.cam_conv_yuv_mode().bits(0);
                    w.cam_conv_data_in_mode().clear_bit();
                    w.cam_conv_data_out_mode().clear_bit();
                    w.cam_conv_protocol_mode().clear_bit();
                    w.cam_conv_8bits_data_inv().clear_bit()
                },
            });
        lcd_cam
            .register_block()
            .cam_ctrl()
            .modify(|_, w| w.cam_update().set_bit());

        let dma = dma.into();
        dma.runtime_ensure_compatible(esp_hal::dma::DmaPeripheral::LCD_CAM);
        Ok(Self {
            lcd_cam,
            dma,
            initial_dma_addr: 0,
        })
    }

    fn connect_input(pin_signal: InputSignal, pin: impl PeripheralInput<'d>) {
        let pin = pin.into();
        pin.apply_input_config(&InputConfig::default());
        pin.set_input_enable(true);
        pin_signal.connect_to(&pin);
    }

    pub fn receive<BUF: DmaRxBuffer>(
        mut self,
        mut buffer: BUF,
    ) -> Result<AsyncCameraTransfer<'d, BUF>, (DmaError, Self, BUF)> {
        self.reset_camera();
        if let Err(error) = self.prepare_dma(&mut buffer) {
            return Err((error, self, buffer));
        }
        self.start_camera();
        Ok(AsyncCameraTransfer {
            camera: ManuallyDrop::new(self),
            buffer_view: ManuallyDrop::new(buffer.into_view()),
            descriptor_empty: false,
        })
    }

    fn prepare_dma<BUF: DmaRxBuffer>(&mut self, buffer: &mut BUF) -> Result<(), DmaError> {
        let preparation = buffer.prepare();
        self.initial_dma_addr = preparation.start as u32;
        compiler_fence(Ordering::SeqCst);
        self.dma.clear_all();
        self.dma.reset();
        self.dma.set_burst_mode(preparation.burst_transfer);
        self.dma.set_descr_burst_mode(true);
        self.dma.set_mem2mem_mode(false);
        self.dma.set_check_owner(preparation.check_owner);
        self.dma.set_link_addr(self.initial_dma_addr);
        self.dma.set_peripheral(LCD_CAM_DMA_ID);
        self.configure_interrupt();
        self.dma.set_async(true);
        self.dma.start();
        compiler_fence(Ordering::SeqCst);
        if self
            .dma
            .pending_interrupts()
            .contains(DmaRxInterrupt::DescriptorError)
        {
            Err(DmaError::DescriptorError)
        } else {
            Ok(())
        }
    }

    fn configure_interrupt(&self) {
        if let (Some(handler), Some(interrupt)) =
            (self.dma.async_handler(), self.dma.peripheral_interrupt())
        {
            for core in esp_hal::system::Cpu::other() {
                esp_hal::interrupt::disable(core, interrupt);
            }
            esp_hal::interrupt::bind_handler(interrupt, handler);
            let interrupts = DmaRxInterrupt::SuccessfulEof
                | DmaRxInterrupt::ErrorEof
                | DmaRxInterrupt::DescriptorEmpty
                | DmaRxInterrupt::DescriptorError
                | DmaRxInterrupt::Done;
            self.dma.unlisten(interrupts);
            self.dma.clear(interrupts);
            esp_hal::interrupt::enable(interrupt, handler.priority());
        }
    }

    fn reset_camera(&self) {
        self.lcd_cam
            .register_block()
            .cam_ctrl1()
            .modify(|_, w| w.cam_reset().set_bit());
        self.lcd_cam
            .register_block()
            .cam_ctrl1()
            .modify(|_, w| w.cam_reset().clear_bit());
        self.lcd_cam
            .register_block()
            .cam_ctrl1()
            .modify(|_, w| w.cam_afifo_reset().set_bit());
        self.lcd_cam
            .register_block()
            .cam_ctrl1()
            .modify(|_, w| w.cam_afifo_reset().clear_bit());
    }

    fn start_camera(&self) {
        self.lcd_cam.register_block().cam_ctrl().modify(|_, w| {
            w.cam_stop_en().set_bit();
            w.cam_update().set_bit()
        });
        self.lcd_cam
            .register_block()
            .cam_ctrl1()
            .modify(|_, w| w.cam_start().set_bit());
    }

    fn resume(&self) {
        self.dma.restart();
        self.start_camera();
    }
}

pub struct AsyncCameraTransfer<'d, BUF: DmaRxBuffer> {
    camera: ManuallyDrop<AsyncCameraDriver<'d>>,
    buffer_view: ManuallyDrop<BUF::View>,
    descriptor_empty: bool,
}

impl<'d, BUF: DmaRxBuffer> AsyncCameraTransfer<'d, BUF> {
    pub async fn wait(&mut self) -> Result<(), DmaError> {
        poll_fn(|context: &mut Context<'_>| {
            let pending = self.camera.dma.pending_interrupts();
            if pending.contains(DmaRxInterrupt::DescriptorEmpty) {
                self.camera.dma.clear(
                    DmaRxInterrupt::DescriptorEmpty
                        | DmaRxInterrupt::Done
                        | DmaRxInterrupt::SuccessfulEof,
                );
                self.descriptor_empty = true;
                Poll::Ready(Ok(()))
            } else if pending.contains(DmaRxInterrupt::DescriptorError)
                || pending.contains(DmaRxInterrupt::ErrorEof)
            {
                self.camera
                    .dma
                    .clear(DmaRxInterrupt::DescriptorError | DmaRxInterrupt::ErrorEof);
                Poll::Ready(Err(DmaError::DescriptorError))
            } else if !pending.is_disjoint(DmaRxInterrupt::SuccessfulEof | DmaRxInterrupt::Done) {
                self.camera
                    .dma
                    .clear(DmaRxInterrupt::SuccessfulEof | DmaRxInterrupt::Done);
                Poll::Ready(Ok(()))
            } else {
                self.camera.dma.waker().register(context.waker());
                self.camera.dma.listen(
                    DmaRxInterrupt::SuccessfulEof
                        | DmaRxInterrupt::ErrorEof
                        | DmaRxInterrupt::DescriptorEmpty
                        | DmaRxInterrupt::DescriptorError
                        | DmaRxInterrupt::Done,
                );
                if !self.camera.dma.pending_interrupts().is_empty() {
                    context.waker().wake_by_ref();
                }
                Poll::Pending
            }
        })
        .await
    }

    pub const fn is_descriptor_empty(&self) -> bool {
        self.descriptor_empty
    }

    pub fn resume_dma(&mut self) {
        self.camera.resume();
    }

    pub fn stop(mut self) -> (AsyncCameraDriver<'d>, BUF::Final) {
        self.stop_peripherals();
        let (camera, view) = self.release();
        (camera, BUF::from_view(view))
    }

    fn stop_peripherals(&mut self) {
        self.camera
            .lcd_cam
            .register_block()
            .cam_ctrl1()
            .modify(|_, w| w.cam_start().clear_bit());
        self.camera.dma.stop();
    }

    fn release(mut self) -> (AsyncCameraDriver<'d>, BUF::View) {
        // SAFETY: self is forgotten immediately, so each ManuallyDrop field is taken once.
        let values = unsafe {
            (
                ManuallyDrop::take(&mut self.camera),
                ManuallyDrop::take(&mut self.buffer_view),
            )
        };
        core::mem::forget(self);
        values
    }
}

impl<BUF: DmaRxBuffer> Deref for AsyncCameraTransfer<'_, BUF> {
    type Target = BUF::View;

    fn deref(&self) -> &Self::Target {
        &self.buffer_view
    }
}

impl<BUF: DmaRxBuffer> DerefMut for AsyncCameraTransfer<'_, BUF> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.buffer_view
    }
}

impl<BUF: DmaRxBuffer> Drop for AsyncCameraTransfer<'_, BUF> {
    fn drop(&mut self) {
        self.stop_peripherals();
        // SAFETY: Drop runs once and neither field is accessed afterwards.
        unsafe {
            ManuallyDrop::drop(&mut self.camera);
            ManuallyDrop::drop(&mut self.buffer_view);
        }
    }
}
