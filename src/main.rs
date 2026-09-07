#![no_std]
#![no_main]

extern crate alloc;

use core::{
    fmt::Write as _,
    sync::atomic::{AtomicBool, Ordering},
};

use alloc::format;
use allocator_api2::vec::Vec;
use embassy_executor::Spawner;
use embassy_net::{Runner, StackResources, tcp::TcpSocket};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use embedded_io_async::Write;
use esp_alloc as _;
use esp_backtrace as _;
use esp_hal::{
    Blocking,
    clock::CpuClock,
    delay::Delay,
    dma::{DmaChannel, DmaRxStreamBuf},
    dma_rx_stream_buffer,
    efuse::{self, InterfaceMacAddress},
    i2c::master::{Config as I2cConfig, I2c},
    i2s::master::{I2s, I2sRx, PdmConfig, PdmRxConfig, PdmSlotMode},
    lcd_cam::{
        ByteOrder,
        cam::{Config as CamConfig, EofMode, VhdeMode, VsyncFilterThreshold},
    },
    ram,
    rng::Rng,
    time::Rate,
    timer::timg::TimerGroup,
    usb::usb_serial_jtag::UsbSerialJtag,
};
use esp_println as _;
use esp_radio::wifi::{
    AuthenticationMethodConfig, Config, ControllerConfig, Interface, WifiController,
    sta::StationConfig,
};
use heapless::String;
use ov3660::{
    AsyncCameraDriver, CameraCapture, CaptureConfig, CaptureEvent, Config as SensorConfig,
    ConverterMode, FrameSize, JpegStreamParser, Ov3660, ParserProgressing,
};

mod flash_layout;
mod ota;
mod provision;

esp_bootloader_esp_idf::esp_app_desc!();

macro_rules! mk_static {
    ($t:ty, $value:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        STATIC_CELL.uninit().write($value)
    }};
}

const MAX_FRAME: usize = 512 * 1024;
const DMA_RING: usize = 20 * 1024;
const DMA_BLOCK: usize = 1024;
const DMA_CHUNK: usize = 4 * 1024;
const HTTP_BOUNDARY: &str = "frameboundary";
const AUDIO_SAMPLE_RATE_HZ: u32 = 16_000;
const AUDIO_DMA_RING: usize = 4092 * 8;
const AUDIO_DMA_BLOCK: usize = 2048;
const AUDIO_CHUNK: usize = 1024;
const AUDIO_BUFFERED_CHUNKS: usize = 8;

struct FrameBuffer {
    data: Vec<u8, &'static esp_alloc::EspHeap>,
    len: usize,
}

struct AudioChunk {
    data: [u8; AUDIO_CHUNK],
    len: usize,
}

static PSRAM_HEAP: esp_alloc::EspHeap = esp_alloc::EspHeap::empty();
static EMPTY_FRAMES: Channel<CriticalSectionRawMutex, FrameBuffer, 2> = Channel::new();
static READY_FRAMES: Channel<CriticalSectionRawMutex, FrameBuffer, 2> = Channel::new();
static PCM_CHUNKS: Channel<CriticalSectionRawMutex, AudioChunk, AUDIO_BUFFERED_CHUNKS> =
    Channel::new();
static AUDIO_ACTIVE: AtomicBool = AtomicBool::new(false);

#[esp_hal::main]
async fn main(spawner: Spawner) -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[ram(reclaimed)] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 36 * 1024);
    let psram = esp_hal::psram::Psram::new(peripherals.PSRAM, Default::default());
    let (psram_start, psram_size) = psram.raw_parts();
    unsafe {
        PSRAM_HEAP.add_region(esp_alloc::HeapRegion::new(
            psram_start,
            psram_size,
            esp_alloc::MemoryCapability::External.into(),
        ));
    }

    for _ in 0..2 {
        let mut data = Vec::with_capacity_in(MAX_FRAME, &PSRAM_HEAP);
        data.resize(MAX_FRAME, 0);
        assert!(EMPTY_FRAMES.try_send(FrameBuffer { data, len: 0 }).is_ok());
    }

    let mut flash = esp_storage::FlashStorage::new(peripherals.FLASH);
    let layout_valid = flash_layout::is_valid(&mut flash);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);
    if !layout_valid {
        defmt::error!("Startup halted: invalid flash or partition layout");
        halt().await;
    }

    let usb = UsbSerialJtag::new(peripherals.USB_DEVICE).into_async();
    let provisioning = provision::load_or_provision(&mut flash, usb).await;

    let camera_config = CamConfig::default()
        .with_frequency(Rate::from_mhz(10))
        .with_eof_mode(EofMode::VsyncSignal)
        .with_vh_de_mode(VhdeMode::De)
        .with_vsync_filter_threshold(VsyncFilterThreshold::Four)
        .with_byte_order(ByteOrder::Native)
        .with_enable_2byte_mode(false);
    let camera = AsyncCameraDriver::new(
        peripherals.LCD_CAM,
        peripherals.DMA_CH0.split().0,
        camera_config,
        ConverterMode::Bypass,
        peripherals.GPIO10,
        peripherals.GPIO13,
        peripherals.GPIO38,
        peripherals.GPIO47,
        peripherals.GPIO15,
        peripherals.GPIO17,
        peripherals.GPIO18,
        peripherals.GPIO16,
        peripherals.GPIO14,
        peripherals.GPIO12,
        peripherals.GPIO11,
        peripherals.GPIO48,
    )
    .unwrap();
    let pdm_config = PdmConfig::rx_only(PdmRxConfig::new_pcm_default(
        Rate::from_hz(AUDIO_SAMPLE_RATE_HZ),
        PdmSlotMode::Mono,
    ));
    let audio = I2s::new_pdm(peripherals.I2S0, peripherals.DMA_CH1, pdm_config)
        .unwrap()
        .into_async()
        .i2s_rx
        .with_clk(peripherals.GPIO42)
        .with_din(peripherals.GPIO41)
        .build();
    let i2c = I2c::new(peripherals.I2C0, I2cConfig::default())
        .unwrap()
        .with_scl(peripherals.GPIO39)
        .with_sda(peripherals.GPIO40);

    let station_config = Config::Station(
        StationConfig::default()
            .with_ssid(provisioning.ssid.as_str().try_into().unwrap())
            .with_authentication(AuthenticationMethodConfig::Wpa2Personal(
                provisioning.password.as_str().try_into().unwrap(),
            )),
    );

    let wifi_interface = Interface::station();
    let controller = WifiController::new(
        peripherals.WIFI,
        ControllerConfig::default().with_initial_config(station_config),
    )
    .unwrap();

    let station_mac = efuse::interface_mac_address(InterfaceMacAddress::Station);
    let station_mac_bytes = station_mac.as_bytes();
    let hostname = format!(
        "esp-cam-{:02x}{:02x}{:02x}",
        station_mac_bytes[3], station_mac_bytes[4], station_mac_bytes[5]
    );
    let mut dhcp_config = embassy_net::DhcpConfig::default();
    dhcp_config.hostname = Some(hostname.as_str().try_into().unwrap());
    let network_config = embassy_net::Config::dhcpv4(dhcp_config);
    let rng = Rng::new();
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;
    let (stack, runner) = embassy_net::new(
        wifi_interface,
        network_config,
        mk_static!(StackResources<6>, StackResources::<6>::new()),
        seed,
    );

    spawner.spawn(connection(controller).unwrap());
    spawner.spawn(net_task(runner).unwrap());
    spawner.spawn(camera_task(camera, i2c).unwrap());
    spawner.spawn(audio_capture_task(audio).unwrap());
    spawner.spawn(http_server(stack, provisioning.track.as_str()).unwrap());
    spawner.spawn(http_server(stack, provisioning.track.as_str()).unwrap());
    spawner.spawn(http_server(stack, provisioning.track.as_str()).unwrap());
    spawner.spawn(ota::ota_task(stack, flash, provisioning, seed).unwrap());

    stack.wait_config_up().await;
    if let Some(config) = stack.config_v4() {
        let address = config.address.address().octets();
        defmt::info!(
            "Acquired IPv4 configuration: {}.{}.{}.{}/{}",
            address[0],
            address[1],
            address[2],
            address[3],
            config.address.prefix_len()
        );
    }

    core::future::pending().await
}

async fn halt() -> ! {
    loop {
        Timer::after(Duration::from_secs(60 * 60)).await;
    }
}

#[embassy_executor::task]
async fn connection(mut controller: WifiController<'static>) {
    loop {
        match controller.connect_async().await {
            Ok(_) => {
                defmt::info!("Wi-Fi connected");
                let reason = controller.wait_for_disconnect_async().await.ok();
                defmt::warn!("Wi-Fi disconnected: {:?}", reason);
            }
            Err(error) => defmt::warn!("Wi-Fi connection failed: {:?}", error),
        }

        Timer::after(Duration::from_secs(5)).await;
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface>) {
    runner.run().await
}

#[embassy_executor::task]
async fn camera_task(camera: AsyncCameraDriver<'static>, i2c: I2c<'static, Blocking>) {
    let mut sensor = Ov3660::new(i2c, 10_000_000);
    let identity = sensor.check_identity().expect("OV3660 not found");
    defmt::info!("OV3660 detected: product ID 0x{:04x}", identity);
    sensor
        .initialize(
            &mut Delay::new(),
            SensorConfig {
                frame_size: FrameSize::FullHd,
                jpeg_quality: 12,
            },
        )
        .expect("OV3660 initialization failed");
    sensor
        .set_vertical_flip(true)
        .expect("OV3660 vertical flip failed");
    sensor.set_brightness(1).expect("OV3660 brightness failed");
    sensor.set_saturation(-2).expect("OV3660 saturation failed");
    defmt::info!("OV3660 initialized: Full HD 1920x1080 JPEG quality 12, XCLK 10 MHz");

    let dma_buffer: DmaRxStreamBuf = dma_rx_stream_buffer!(DMA_RING, DMA_BLOCK);
    let mut capture = CameraCapture::new(camera, dma_buffer, CaptureConfig::default());
    capture.start().expect("camera DMA start failed");

    let mut frame = EMPTY_FRAMES.receive().await;
    let mut parser = JpegStreamParser::new();
    let mut chunk = [0u8; DMA_CHUNK];
    let mut frames = 0u32;
    let mut dropped = 0u32;
    let mut errors = 0u32;
    let mut bytes = 0u64;
    let mut metrics_at = Instant::now();

    loop {
        let result = capture.next_chunk(&mut chunk).await;
        if result.event != CaptureEvent::None {
            errors = errors.saturating_add(1);
        }
        if result.resyncing {
            parser.reset_state();
            report_capture_metrics(
                &mut metrics_at,
                &mut frames,
                &mut dropped,
                &mut errors,
                &mut bytes,
            );
            continue;
        }
        if result.len == 0 {
            continue;
        }

        parser.reset_input_offset();
        loop {
            match parser.parse(&chunk[..result.len], frame.data.as_mut_slice()) {
                Ok(ParserProgressing::InputBufferEmpty) => break,
                Ok(ParserProgressing::EndOfImage) => {
                    let len = parser.bytes_written();
                    ota::CAMERA_HEALTHY.signal(());
                    frames = frames.saturating_add(1);
                    bytes = bytes.saturating_add(len as u64);
                    if let Ok(next) = EMPTY_FRAMES
                        .try_receive()
                        .or_else(|_| READY_FRAMES.try_receive())
                    {
                        frame.len = len;
                        assert!(READY_FRAMES.try_send(frame).is_ok());
                        frame = next;
                    } else {
                        dropped = dropped.saturating_add(1);
                    }
                    parser.reset_output_offset();
                }
                Ok(ParserProgressing::OutputBufferFull) => {
                    dropped = dropped.saturating_add(1);
                    parser.reset_state();
                    break;
                }
                Err(_) => {
                    errors = errors.saturating_add(1);
                    break;
                }
            }
        }

        report_capture_metrics(
            &mut metrics_at,
            &mut frames,
            &mut dropped,
            &mut errors,
            &mut bytes,
        );
    }
}

#[embassy_executor::task]
async fn audio_capture_task(audio: I2sRx<'static, esp_hal::Async>) {
    let dma_buffer = dma_rx_stream_buffer!(AUDIO_DMA_RING, AUDIO_DMA_BLOCK);
    let mut transaction = audio.read(dma_buffer).unwrap();

    loop {
        if transaction.wait_for_available_async().await.is_err() {
            // Descriptor exhaustion stops DMA; reset the transfer with its reusable buffer.
            let (mut audio, mut dma_buffer) = transaction.stop();
            loop {
                match audio.read(dma_buffer) {
                    Ok(restarted) => {
                        transaction = restarted;
                        break;
                    }
                    Err((_, returned_audio, returned_dma_buffer)) => {
                        audio = returned_audio;
                        dma_buffer = returned_dma_buffer;
                        Timer::after(Duration::from_millis(100)).await;
                    }
                }
            }
            continue;
        }
        while transaction.available_bytes() > 0 {
            let mut chunk = AudioChunk {
                data: [0; AUDIO_CHUNK],
                len: 0,
            };
            chunk.len = transaction.pop(&mut chunk.data) & !1;
            if chunk.len == 0 {
                break;
            }

            // The producer never waits: when full, drop the newest PCM chunk.
            let _ = PCM_CHUNKS.try_send(chunk);
        }
    }
}

fn report_capture_metrics(
    metrics_at: &mut Instant,
    frames: &mut u32,
    dropped: &mut u32,
    errors: &mut u32,
    bytes: &mut u64,
) {
    let elapsed = metrics_at.elapsed();
    if elapsed < Duration::from_secs(10) {
        return;
    }
    let seconds = elapsed.as_secs().max(1);
    defmt::info!(
        "capture: {} fps, {} kbps, {} dropped, {} errors",
        *frames as u64 / seconds,
        (*bytes * 8) / (seconds * 1024),
        *dropped,
        *errors
    );
    *frames = 0;
    *dropped = 0;
    *errors = 0;
    *bytes = 0;
    *metrics_at = Instant::now();
}

#[embassy_executor::task(pool_size = 3)]
async fn http_server(stack: embassy_net::Stack<'static>, track: &'static str) {
    let mut rx_buffer = [0u8; 1024];
    let mut tx_buffer = [0u8; 4 * 1024];
    loop {
        stack.wait_config_up().await;
        if let Some(config) = stack.config_v4() {
            let address = config.address.address().octets();
            defmt::info!(
                "HTTP server listening at http://{}.{}.{}.{}:80",
                address[0],
                address[1],
                address[2],
                address[3]
            );
        }

        let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(Duration::from_secs(30)));
        if socket.accept(80).await.is_err() {
            defmt::warn!("HTTP accept failed");
            Timer::after(Duration::from_millis(250)).await;
            continue;
        }
        if handle_http_connection(&mut socket, track).await.is_err() {
            defmt::warn!("HTTP connection closed");
            socket.abort();
            let _ = with_timeout(Duration::from_secs(1), socket.flush()).await;
            continue;
        }
        let _ = socket.flush().await;
        socket.close();
        let _ = with_timeout(Duration::from_secs(2), socket.flush()).await;
    }
}

async fn handle_http_connection(
    socket: &mut TcpSocket<'_>,
    track: &'static str,
) -> Result<(), embassy_net::tcp::Error> {
    if ota::TRANSFER_ACTIVE.load(core::sync::atomic::Ordering::Acquire) {
        return socket
            .write_all(
                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await;
    }
    let mut request = [0u8; 512];
    let mut used = 0usize;
    while used < request.len() && !request[..used].windows(4).any(|part| part == b"\r\n\r\n") {
        let read = socket.read(&mut request[used..]).await?;
        if read == 0 {
            return Ok(());
        }
        used += read;
    }

    let line_end = request[..used]
        .windows(2)
        .position(|part| part == b"\r\n")
        .unwrap_or(used);
    match &request[..line_end] {
        b"GET /stream HTTP/1.0" | b"GET /stream HTTP/1.1" => stream_mjpeg(socket).await,
        b"GET /capture.jpg HTTP/1.0" | b"GET /capture.jpg HTTP/1.1" => send_snapshot(socket).await,
        b"GET /status HTTP/1.0" | b"GET /status HTTP/1.1" => send_status(socket, track).await,
        b"GET /audio.pcm HTTP/1.0" | b"GET /audio.pcm HTTP/1.1" => {
            if AUDIO_ACTIVE
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return socket
                    .write_all(
                        b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
            }
            let result = stream_audio(socket).await;
            AUDIO_ACTIVE.store(false, Ordering::Release);
            result
        }
        _ => {
            socket
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
        }
    }
}

async fn send_status(
    socket: &mut TcpSocket<'_>,
    track: &'static str,
) -> Result<(), embassy_net::tcp::Error> {
    let transfer_active = ota::TRANSFER_ACTIVE.load(core::sync::atomic::Ordering::Acquire);
    let mut body: String<128> = String::new();
    let _ = write!(
        body,
        "{{\"version\":\"{}\",\"track\":\"{}\",\"transfer_active\":{}}}",
        env!("CARGO_PKG_VERSION"),
        track,
        transfer_active
    );
    let mut headers: String<160> = String::new();
    let _ = write!(
        headers,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    socket.write_all(headers.as_bytes()).await?;
    socket.write_all(body.as_bytes()).await
}

async fn stream_mjpeg(socket: &mut TcpSocket<'_>) -> Result<(), embassy_net::tcp::Error> {
    let mut headers: String<192> = String::new();
    let _ = write!(
        headers,
        "HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary={}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        HTTP_BOUNDARY
    );
    socket.write_all(headers.as_bytes()).await?;

    loop {
        if ota::TRANSFER_ACTIVE.load(core::sync::atomic::Ordering::Acquire) {
            socket
                .write_all(format!("--{}--\r\n", HTTP_BOUNDARY).as_bytes())
                .await?;
            return Ok(());
        }
        let frame = match with_timeout(Duration::from_secs(5), READY_FRAMES.receive()).await {
            Ok(frame) => frame,
            Err(_) => {
                socket
                    .write_all(format!("--{}--\r\n", HTTP_BOUNDARY).as_bytes())
                    .await?;
                return Ok(());
            }
        };
        let mut part: String<128> = String::new();
        let _ = write!(
            part,
            "--{}\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
            HTTP_BOUNDARY, frame.len
        );
        let result = async {
            socket.write_all(part.as_bytes()).await?;
            socket.write_all(&frame.data[..frame.len]).await?;
            socket.write_all(b"\r\n").await
        }
        .await;
        assert!(EMPTY_FRAMES.try_send(frame).is_ok());
        result?;
    }
}

async fn stream_audio(socket: &mut TcpSocket<'_>) -> Result<(), embassy_net::tcp::Error> {
    socket
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        )
        .await?;

    loop {
        if ota::TRANSFER_ACTIVE.load(Ordering::Acquire) {
            return Ok(());
        }
        let chunk = match with_timeout(Duration::from_secs(1), PCM_CHUNKS.receive()).await {
            Ok(chunk) => chunk,
            Err(_) => continue,
        };
        socket.write_all(&chunk.data[..chunk.len]).await?;
    }
}

async fn send_snapshot(socket: &mut TcpSocket<'_>) -> Result<(), embassy_net::tcp::Error> {
    let frame = match with_timeout(Duration::from_secs(5), READY_FRAMES.receive()).await {
        Ok(frame) => frame,
        Err(_) => {
            return socket
                .write_all(
                    b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
        }
    };
    let mut headers: String<160> = String::new();
    let _ = write!(
        headers,
        "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        frame.len
    );
    let result = async {
        socket.write_all(headers.as_bytes()).await?;
        socket.write_all(&frame.data[..frame.len]).await
    }
    .await;
    assert!(EMPTY_FRAMES.try_send(frame).is_ok());
    result
}
