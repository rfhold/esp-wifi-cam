#![no_std]

pub mod jpeg;
mod registers;
mod settings;

#[cfg(feature = "esp32s3")]
mod capture;
#[cfg(feature = "esp32s3")]
mod driver;

#[cfg(feature = "esp32s3")]
pub use capture::{CameraCapture, CaptureConfig, CaptureEvent, ChunkResult};
#[cfg(feature = "esp32s3")]
pub use driver::{AsyncCameraDriver, AsyncCameraTransfer, ConverterMode};
pub use jpeg::{JpegParserError, JpegParserState, JpegStreamParser, ParserProgressing};

use embedded_hal::{delay::DelayNs, i2c::I2c};
use registers::*;
use settings::{DEFAULT_REGISTERS, JPEG_FORMAT, SATURATION_LEVELS, Setting};

pub const SCCB_ADDRESS: u8 = 0x3c;
pub const PRODUCT_ID: u16 = 0x3660;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameSize {
    Qvga,
    Vga,
    Svga,
    Hd,
    FullHd,
    Uxga,
    Qxga,
}

impl FrameSize {
    pub const fn dimensions(self) -> (u16, u16) {
        match self {
            Self::Qvga => (320, 240),
            Self::Vga => (640, 480),
            Self::Svga => (800, 600),
            Self::Hd => (1280, 720),
            Self::FullHd => (1920, 1080),
            Self::Uxga => (1600, 1200),
            Self::Qxga => (2048, 1536),
        }
    }
}

impl TryFrom<(u16, u16)> for FrameSize {
    type Error = ConfigError;

    fn try_from(dimensions: (u16, u16)) -> Result<Self, Self::Error> {
        match dimensions {
            (320, 240) => Ok(Self::Qvga),
            (640, 480) => Ok(Self::Vga),
            (800, 600) => Ok(Self::Svga),
            (1280, 720) => Ok(Self::Hd),
            (1920, 1080) => Ok(Self::FullHd),
            (1600, 1200) => Ok(Self::Uxga),
            (2048, 1536) => Ok(Self::Qxga),
            (width, height) => Err(ConfigError::UnsupportedFrameSize { width, height }),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Config {
    pub frame_size: FrameSize,
    pub jpeg_quality: u8,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            frame_size: FrameSize::Vga,
            jpeg_quality: 10,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigError {
    UnsupportedFrameSize { width: u16, height: u16 },
    InvalidQuality(u8),
    InvalidBrightness(i8),
    InvalidSaturation(i8),
}

#[derive(Debug, Eq, PartialEq)]
pub enum Error<E> {
    I2c(E),
    UnexpectedProductId(u16),
    InvalidConfiguration(ConfigError),
}

#[derive(Clone, Copy)]
struct Geometry {
    max_width: u16,
    max_height: u16,
    start_x: u16,
    start_y: u16,
    end_x: u16,
    end_y: u16,
    total_x: u16,
    total_y: u16,
}

const RATIO_4X3: Geometry = Geometry {
    max_width: 2048,
    max_height: 1536,
    start_x: 0,
    start_y: 0,
    end_x: 2079,
    end_y: 1547,
    total_x: 2300,
    total_y: 1564,
};

const RATIO_16X9: Geometry = Geometry {
    max_width: 1920,
    max_height: 1080,
    start_x: 64,
    start_y: 242,
    end_x: 2015,
    end_y: 1333,
    total_x: 2172,
    total_y: 1322,
};

pub struct Ov3660<I2C> {
    i2c: I2C,
    xclk_hz: u32,
    frame_size: FrameSize,
    binning: bool,
    vertical_flip: bool,
}

impl<I2C> Ov3660<I2C> {
    pub const fn new(i2c: I2C, xclk_hz: u32) -> Self {
        Self {
            i2c,
            xclk_hz,
            frame_size: FrameSize::Vga,
            binning: false,
            vertical_flip: false,
        }
    }

    pub fn release(self) -> I2C {
        self.i2c
    }
}

impl<I2C> Ov3660<I2C>
where
    I2C: I2c,
{
    pub fn check_identity(&mut self) -> Result<u16, Error<I2C::Error>> {
        let high = self.read_register(PID_HIGH)?;
        let low = self.read_register(PID_LOW)?;
        let product_id = u16::from_be_bytes([high, low]);
        if product_id == PRODUCT_ID {
            Ok(product_id)
        } else {
            Err(Error::UnexpectedProductId(product_id))
        }
    }

    pub fn initialize<D>(&mut self, delay: &mut D, config: Config) -> Result<(), Error<I2C::Error>>
    where
        D: DelayNs,
    {
        if config.jpeg_quality > 63 {
            return Err(Error::InvalidConfiguration(ConfigError::InvalidQuality(
                config.jpeg_quality,
            )));
        }

        self.check_identity()?;
        self.write_register(SYSTEM_CONTROL0, 0x82)?;
        delay.delay_ms(100);
        self.write_settings(delay, DEFAULT_REGISTERS)?;
        self.set_auto_exposure_level_zero()?;
        delay.delay_ms(100);

        for &(register, value) in JPEG_FORMAT {
            self.write_register(register, value)?;
        }

        self.set_frame_size(config.frame_size)?;
        self.set_quality(config.jpeg_quality)?;
        Ok(())
    }

    pub fn set_frame_size(&mut self, frame_size: FrameSize) -> Result<(), Error<I2C::Error>> {
        let (width, height) = frame_size.dimensions();
        let geometry = match frame_size {
            FrameSize::Hd | FrameSize::FullHd => RATIO_16X9,
            _ => RATIO_4X3,
        };
        let binning = width <= geometry.max_width / 2 && height <= geometry.max_height / 2;
        let scale = !((width == geometry.max_width && height == geometry.max_height)
            || (width == geometry.max_width / 2 && height == geometry.max_height / 2));

        self.write_xy(X_ADDR_START, geometry.start_x, geometry.start_y)?;
        self.write_xy(X_ADDR_END, geometry.end_x, geometry.end_y)?;
        self.write_xy(X_OUTPUT_SIZE, width, height)?;
        if binning {
            self.write_xy(X_TOTAL_SIZE, geometry.total_x, geometry.total_y / 2 + 1)?;
            self.write_xy(X_OFFSET, 8, 2)?;
        } else {
            self.write_xy(X_TOTAL_SIZE, geometry.total_x, geometry.total_y)?;
            self.write_xy(X_OFFSET, 16, 6)?;
        }
        self.set_register_bits(ISP_CONTROL_01, 0x20, scale)?;
        self.set_image_options(binning, self.vertical_flip)?;
        self.set_jpeg_pll(frame_size)?;

        self.frame_size = frame_size;
        self.binning = binning;
        Ok(())
    }

    pub fn set_quality(&mut self, quality: u8) -> Result<(), Error<I2C::Error>> {
        if quality > 63 {
            return Err(Error::InvalidConfiguration(ConfigError::InvalidQuality(
                quality,
            )));
        }
        self.write_register(COMPRESSION_CTRL07, quality)
    }

    pub fn set_vertical_flip(&mut self, enabled: bool) -> Result<(), Error<I2C::Error>> {
        self.set_image_options(self.binning, enabled)?;
        self.vertical_flip = enabled;
        Ok(())
    }

    pub fn set_brightness(&mut self, level: i8) -> Result<(), Error<I2C::Error>> {
        if !(-3..=3).contains(&level) {
            return Err(Error::InvalidConfiguration(ConfigError::InvalidBrightness(
                level,
            )));
        }
        let value = level.unsigned_abs() << 4;
        self.write_register(0x5587, value)?;
        self.set_register_bits(0x5588, 0x08, level < 0)
    }

    pub fn set_saturation(&mut self, level: i8) -> Result<(), Error<I2C::Error>> {
        if !(-4..=4).contains(&level) {
            return Err(Error::InvalidConfiguration(ConfigError::InvalidSaturation(
                level,
            )));
        }
        let values = &SATURATION_LEVELS[(level + 4) as usize];
        for (offset, &value) in values.iter().enumerate() {
            self.write_register(0x5381 + offset as u16, value)?;
        }
        Ok(())
    }

    fn write_settings<D>(
        &mut self,
        delay: &mut D,
        settings: &[Setting],
    ) -> Result<(), Error<I2C::Error>>
    where
        D: DelayNs,
    {
        for setting in settings {
            match *setting {
                Setting::Write(register, value) => self.write_register(register, value)?,
                Setting::DelayMs(milliseconds) => delay.delay_ms(milliseconds),
            }
        }
        Ok(())
    }

    fn set_auto_exposure_level_zero(&mut self) -> Result<(), Error<I2C::Error>> {
        for &(register, value) in &[
            (0x3a0f, 0x3b),
            (0x3a10, 0x32),
            (0x3a1b, 0x3b),
            (0x3a1e, 0x32),
            (0x3a11, 0x76),
            (0x3a1f, 0x19),
        ] {
            self.write_register(register, value)?;
        }
        Ok(())
    }

    fn set_image_options(
        &mut self,
        binning: bool,
        vertical_flip: bool,
    ) -> Result<(), Error<I2C::Error>> {
        let mut reg20 = if binning { 0x01 } else { 0x40 };
        let reg21 = if binning { 0x21 } else { 0x20 };
        if vertical_flip {
            reg20 |= 0x06;
        }
        let reg4514 = match (binning, vertical_flip) {
            (true, false) => 0xaa,
            (true, true) => 0xbb,
            (false, _) => 0x88,
        };

        self.write_register(TIMING_TC_REG20, reg20)?;
        self.write_register(TIMING_TC_REG21, reg21)?;
        self.write_register(0x4514, reg4514)?;
        if binning {
            self.write_register(0x4520, 0x0b)?;
            self.write_register(X_INCREMENT, 0x31)?;
            self.write_register(Y_INCREMENT, 0x31)?;
        } else {
            self.write_register(0x4520, 0xb0)?;
            self.write_register(X_INCREMENT, 0x11)?;
            self.write_register(Y_INCREMENT, 0x11)?;
        }
        Ok(())
    }

    fn set_jpeg_pll(&mut self, frame_size: FrameSize) -> Result<(), Error<I2C::Error>> {
        let (multiplier, pclk_divider) =
            if frame_size == FrameSize::Qxga || self.xclk_hz == 16_000_000 {
                (24, 8)
            } else {
                (30, 10)
            };
        self.write_register(SC_PLLS_CTRL0, 0x00)?;
        self.write_register(SC_PLLS_CTRL1, multiplier)?;
        self.write_register(SC_PLLS_CTRL2, 0x11)?;
        self.write_register(SC_PLLS_CTRL3, 0x30)?;
        self.write_register(PCLK_RATIO, pclk_divider)?;
        self.write_register(VFIFO_CTRL0C, 0x22)
    }

    fn write_xy(&mut self, register: u16, x: u16, y: u16) -> Result<(), Error<I2C::Error>> {
        self.write_u16(register, x)?;
        self.write_u16(register + 2, y)
    }

    fn write_u16(&mut self, register: u16, value: u16) -> Result<(), Error<I2C::Error>> {
        let [high, low] = value.to_be_bytes();
        self.write_register(register, high)?;
        self.write_register(register + 1, low)
    }

    fn set_register_bits(
        &mut self,
        register: u16,
        mask: u8,
        enabled: bool,
    ) -> Result<(), Error<I2C::Error>> {
        let current = self.read_register(register)?;
        let value = if enabled {
            current | mask
        } else {
            current & !mask
        };
        self.write_register(register, value)
    }

    fn read_register(&mut self, register: u16) -> Result<u8, Error<I2C::Error>> {
        let address = register.to_be_bytes();
        let mut value = [0];
        self.i2c
            .write_read(SCCB_ADDRESS, &address, &mut value)
            .map_err(Error::I2c)?;
        Ok(value[0])
    }

    fn write_register(&mut self, register: u16, value: u8) -> Result<(), Error<I2C::Error>> {
        let [high, low] = register.to_be_bytes();
        self.i2c
            .write(SCCB_ADDRESS, &[high, low, value])
            .map_err(Error::I2c)
    }
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_hal::i2c::{ErrorKind, ErrorType, Operation};
    use std::{collections::BTreeMap, vec, vec::Vec};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct MockError;

    impl embedded_hal::i2c::Error for MockError {
        fn kind(&self) -> ErrorKind {
            ErrorKind::Other
        }
    }

    struct MockI2c {
        registers: BTreeMap<u16, u8>,
        writes: Vec<Vec<u8>>,
    }

    impl MockI2c {
        fn ov3660() -> Self {
            Self {
                registers: BTreeMap::from([(PID_HIGH, 0x36), (PID_LOW, 0x60)]),
                writes: Vec::new(),
            }
        }

        fn has_write(&self, register: u16, value: u8) -> bool {
            let [high, low] = register.to_be_bytes();
            self.writes.iter().any(|write| write == &[high, low, value])
        }
    }

    impl ErrorType for MockI2c {
        type Error = MockError;
    }

    impl I2c for MockI2c {
        fn transaction(
            &mut self,
            address: u8,
            operations: &mut [Operation<'_>],
        ) -> Result<(), Self::Error> {
            assert_eq!(address, SCCB_ADDRESS);
            let mut requested_register = None;
            for operation in operations {
                match operation {
                    Operation::Write(bytes) => {
                        self.writes.push(bytes.to_vec());
                        match **bytes {
                            [high, low] => {
                                requested_register = Some(u16::from_be_bytes([high, low]));
                            }
                            [high, low, value] => {
                                self.registers
                                    .insert(u16::from_be_bytes([high, low]), value);
                            }
                            _ => return Err(MockError),
                        }
                    }
                    Operation::Read(bytes) => {
                        let register = requested_register.ok_or(MockError)?;
                        if bytes.len() != 1 {
                            return Err(MockError);
                        }
                        bytes[0] = *self.registers.get(&register).unwrap_or(&0);
                    }
                }
            }
            Ok(())
        }
    }

    #[derive(Default)]
    struct MockDelay {
        nanoseconds: Vec<u32>,
    }

    impl DelayNs for MockDelay {
        fn delay_ns(&mut self, nanoseconds: u32) {
            self.nanoseconds.push(nanoseconds);
        }
    }

    #[test]
    fn checks_pid_with_16_bit_register_addresses() {
        let mut sensor = Ov3660::new(MockI2c::ov3660(), 20_000_000);
        assert_eq!(sensor.check_identity(), Ok(PRODUCT_ID));
        let i2c = sensor.release();
        assert_eq!(i2c.writes, vec![vec![0x30, 0x0a], vec![0x30, 0x0b]]);
    }

    #[test]
    fn rejects_an_unexpected_pid() {
        let mut i2c = MockI2c::ov3660();
        i2c.registers.insert(PID_LOW, 0x61);
        let mut sensor = Ov3660::new(i2c, 20_000_000);
        assert_eq!(
            sensor.check_identity(),
            Err(Error::UnexpectedProductId(0x3661))
        );
    }

    #[test]
    fn initialization_orders_reset_delays_jpeg_geometry_and_quality() {
        let mut sensor = Ov3660::new(MockI2c::ov3660(), 20_000_000);
        let mut delay = MockDelay::default();
        sensor
            .initialize(
                &mut delay,
                Config {
                    frame_size: FrameSize::Vga,
                    jpeg_quality: 12,
                },
            )
            .unwrap();
        let i2c = sensor.release();

        assert_eq!(
            delay.nanoseconds,
            vec![100_000_000, 10_000_000, 100_000_000]
        );
        assert_eq!(i2c.writes[2], vec![0x30, 0x08, 0x82]);
        assert_eq!(i2c.writes[3], vec![0x30, 0x08, 0x82]);
        let jpeg = i2c
            .writes
            .iter()
            .position(|write| write == &[0x47, 0x1c, 0x50])
            .unwrap();
        let geometry = i2c
            .writes
            .iter()
            .position(|write| write == &[0x38, 0x08, 0x02])
            .unwrap();
        let quality = i2c
            .writes
            .iter()
            .position(|write| write == &[0x44, 0x07, 12])
            .unwrap();
        assert!(jpeg < geometry && geometry < quality);
        assert!(i2c.has_write(0x3006, 0xff));
    }

    #[test]
    fn programs_vga_and_hd_geometry_and_pll() {
        let mut sensor = Ov3660::new(MockI2c::ov3660(), 20_000_000);
        sensor.set_frame_size(FrameSize::Vga).unwrap();
        sensor.set_frame_size(FrameSize::Hd).unwrap();
        let i2c = sensor.release();

        assert!(i2c.has_write(X_OUTPUT_SIZE, 0x02));
        assert!(i2c.has_write(X_OUTPUT_SIZE + 1, 0x80));
        assert!(i2c.has_write(X_OUTPUT_SIZE + 2, 0x01));
        assert!(i2c.has_write(X_OUTPUT_SIZE + 3, 0xe0));
        assert!(i2c.has_write(X_ADDR_START, 0x00));
        assert!(i2c.has_write(X_ADDR_START + 1, 0x40));
        assert!(i2c.has_write(X_ADDR_START + 2, 0x00));
        assert!(i2c.has_write(X_ADDR_START + 3, 0xf2));
        assert!(i2c.has_write(X_OUTPUT_SIZE, 0x05));
        assert!(i2c.has_write(X_OUTPUT_SIZE + 1, 0x00));
        assert!(i2c.has_write(X_OUTPUT_SIZE + 2, 0x02));
        assert!(i2c.has_write(X_OUTPUT_SIZE + 3, 0xd0));
        assert!(i2c.has_write(SC_PLLS_CTRL1, 30));
        assert!(i2c.has_write(PCLK_RATIO, 10));
    }

    #[test]
    fn programs_full_hd_output_and_16x9_crop() {
        assert_eq!(FrameSize::FullHd.dimensions(), (1920, 1080));
        assert_eq!(FrameSize::try_from((1920, 1080)), Ok(FrameSize::FullHd));

        let mut sensor = Ov3660::new(MockI2c::ov3660(), 20_000_000);
        sensor.set_frame_size(FrameSize::FullHd).unwrap();
        let i2c = sensor.release();

        for (register, value) in [
            (X_ADDR_START, 0x00),
            (X_ADDR_START + 1, 0x40),
            (X_ADDR_START + 2, 0x00),
            (X_ADDR_START + 3, 0xf2),
            (X_ADDR_END, 0x07),
            (X_ADDR_END + 1, 0xdf),
            (X_ADDR_END + 2, 0x05),
            (X_ADDR_END + 3, 0x35),
            (X_OUTPUT_SIZE, 0x07),
            (X_OUTPUT_SIZE + 1, 0x80),
            (X_OUTPUT_SIZE + 2, 0x04),
            (X_OUTPUT_SIZE + 3, 0x38),
        ] {
            assert!(i2c.has_write(register, value));
        }
    }

    #[test]
    fn applies_seeed_flip_brightness_and_saturation_tuning() {
        let mut sensor = Ov3660::new(MockI2c::ov3660(), 20_000_000);
        sensor.set_vertical_flip(true).unwrap();
        sensor.set_brightness(1).unwrap();
        sensor.set_saturation(-2).unwrap();
        let i2c = sensor.release();

        assert!(i2c.has_write(TIMING_TC_REG20, 0x46));
        assert!(i2c.has_write(0x5587, 0x10));
        assert!(i2c.has_write(0x5588, 0x00));
        for (offset, &value) in SATURATION_LEVELS[2].iter().enumerate() {
            assert!(i2c.has_write(0x5381 + offset as u16, value));
        }
    }

    #[test]
    fn rejects_invalid_configuration_values() {
        assert_eq!(
            FrameSize::try_from((123, 456)),
            Err(ConfigError::UnsupportedFrameSize {
                width: 123,
                height: 456,
            })
        );
        let mut sensor = Ov3660::new(MockI2c::ov3660(), 20_000_000);
        assert_eq!(
            sensor.set_quality(64),
            Err(Error::InvalidConfiguration(ConfigError::InvalidQuality(64)))
        );
        assert_eq!(
            sensor.set_brightness(4),
            Err(Error::InvalidConfiguration(ConfigError::InvalidBrightness(
                4
            )))
        );
        assert_eq!(
            sensor.set_saturation(-5),
            Err(Error::InvalidConfiguration(ConfigError::InvalidSaturation(
                -5
            )))
        );
    }
}
// Modified from the attributed upstream implementations for this crate.
