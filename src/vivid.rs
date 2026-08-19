use std::{
    io,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

use image::RgbaImage;
use vivid_protocol::{
    cbor::Value,
    media::{self, RasterDeltaOperation},
    resource::Resource,
    track::{KindConfiguration, RasterConfiguration, TrackConfiguration, TrackMode},
};
use vivid_sdk::{
    ChannelEvent, CoordinateModel, Fit, LaneClass, MILESTONE_OUTPUT_READY, ProducerAuthentication,
    ProducerConfig, RequestMetadata, SceneNode, Session, SessionEvent, SlotBinding, Surface,
    SurfaceDefinition, SurfaceDescriptor, SurfaceRole, TERMINAL_SURFACE, Track, TrackChannel,
    TrackWaitCondition,
};

use crate::terminal::{Layout, RESERVED_UI_ROWS};

const SLOT_RASTER: u64 = 3;
const FRAME_RATE: u64 = 30;
const READY_TIMEOUT_US: u64 = 5_000_000;

#[derive(Debug)]
pub enum Event {
    Ready(Layout),
    Presented,
    Resized(Layout),
    Error(String),
    Closed,
}

enum Command {
    Shutdown,
}

#[derive(Clone)]
struct Frame {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

pub struct VividHandle {
    commands: SyncSender<Command>,
    events: Receiver<Event>,
    latest: Arc<Mutex<Option<Frame>>>,
    join: Option<thread::JoinHandle<()>>,
}

impl VividHandle {
    pub fn spawn(config: ProducerConfig, resolution_scale: f32) -> io::Result<Self> {
        let (command_tx, command_rx) = mpsc::sync_channel(1);
        let (event_tx, event_rx) = mpsc::channel();
        let latest = Arc::new(Mutex::new(None));
        let worker_latest = latest.clone();
        let join = thread::Builder::new()
            .name("vvpaint-vivid".into())
            .spawn(move || {
                let result = Worker::connect(config, resolution_scale, event_tx.clone())
                    .and_then(|worker| worker.run(command_rx, worker_latest));
                if let Err(error) = result {
                    let _ = event_tx.send(Event::Error(error.to_string()));
                }
                let _ = event_tx.send(Event::Closed);
            })?;
        Ok(Self {
            commands: command_tx,
            events: event_rx,
            latest,
            join: Some(join),
        })
    }

    pub fn wait_ready(&self) -> io::Result<Layout> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let timeout = deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(timeout) {
                Ok(Event::Ready(layout)) => return Ok(layout),
                Ok(Event::Error(error)) => return Err(io::Error::other(error)),
                Ok(Event::Closed) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "Vivid worker closed during startup",
                    ));
                }
                Ok(_) => {}
                Err(error) => return Err(io::Error::new(io::ErrorKind::TimedOut, error)),
            }
        }
    }

    pub fn wait_presented(&self) -> io::Result<()> {
        loop {
            match self.events.recv_timeout(Duration::from_secs(10)) {
                Ok(Event::Presented) => return Ok(()),
                Ok(Event::Error(error)) => return Err(io::Error::other(error)),
                Ok(Event::Closed) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "Vivid worker closed before presentation",
                    ));
                }
                Ok(_) => {}
                Err(error) => return Err(io::Error::new(io::ErrorKind::TimedOut, error)),
            }
        }
    }

    pub fn publish(&self, image: RgbaImage) -> io::Result<()> {
        let frame = Frame {
            width: image.width(),
            height: image.height(),
            pixels: image.into_raw(),
        };
        *self
            .latest
            .lock()
            .map_err(|_| io::Error::other("frame mailbox is poisoned"))? = Some(frame);
        Ok(())
    }

    pub fn try_event(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }

    pub fn shutdown(mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for VividHandle {
    fn drop(&mut self) {
        let _ = self.commands.try_send(Command::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

pub fn producer_config() -> ProducerConfig {
    ProducerConfig {
        authentication: ProducerAuthentication::RootFromEnvironment,
        producer_name: "vvpaint".into(),
        producer_version: env!("CARGO_PKG_VERSION").into(),
        target_profile: vivid_sdk::TERMINAL_SURFACE.into(),
        required_profiles: vec![
            vivid_sdk::LIVE_MEDIA.into(),
            vivid_sdk::TERMINAL_SURFACE.into(),
            vivid_sdk::CORE_CONTROL.into(),
        ],
        optional_profiles: vec![],
        ..ProducerConfig::default()
    }
}

struct RasterState {
    track: Track,
    channel: TrackChannel,
    width: u32,
    height: u32,
    frame_id: u64,
    delta: bool,
    zstd: bool,
}

struct Worker {
    session: Session,
    surface: Option<Surface>,
    node: Option<SceneNode>,
    raster: Option<RasterState>,
    layout: Layout,
    resolution_scale: f32,
    last_pixels: Option<Vec<u8>>,
    force_full: bool,
    events: Sender<Event>,
}

impl Worker {
    fn connect(
        config: ProducerConfig,
        resolution_scale: f32,
        events: Sender<Event>,
    ) -> io::Result<Self> {
        let mut session = Session::connect(config).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("{error}; run vvpaint inside a Vivid 1.5 terminal presenter"),
            )
        })?;
        if session.info().target_profile != TERMINAL_SURFACE {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "presenter did not select terminal-surface-v1",
            ));
        }
        let layout = wait_for_layout(&mut session, resolution_scale)?;
        events.send(Event::Ready(layout)).map_err(|_| {
            io::Error::new(io::ErrorKind::BrokenPipe, "UI closed during Vivid startup")
        })?;
        Ok(Self {
            session,
            surface: None,
            node: None,
            raster: None,
            layout,
            resolution_scale,
            last_pixels: None,
            force_full: true,
            events,
        })
    }

    fn run(
        mut self,
        commands: Receiver<Command>,
        latest: Arc<Mutex<Option<Frame>>>,
    ) -> io::Result<()> {
        let initial = self.wait_for_initial_frame(&commands, &latest)?;
        self.initialize(initial)?;
        self.events.send(Event::Presented).map_err(|_| {
            io::Error::new(io::ErrorKind::BrokenPipe, "UI closed during presentation")
        })?;
        let mut last_send = Instant::now();
        loop {
            if matches!(
                commands.try_recv(),
                Ok(Command::Shutdown) | Err(mpsc::TryRecvError::Disconnected)
            ) {
                break;
            }
            self.poll_session()?;
            self.poll_channel()?;
            if last_send.elapsed() >= Duration::from_millis(33)
                && let Some(frame) = take_latest(&latest)?
            {
                self.submit(frame)?;
                last_send = Instant::now();
            }
            thread::sleep(Duration::from_millis(3));
        }
        self.teardown()
    }

    fn wait_for_initial_frame(
        &self,
        commands: &Receiver<Command>,
        latest: &Arc<Mutex<Option<Frame>>>,
    ) -> io::Result<Frame> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(frame) = take_latest(latest)? {
                return Ok(frame);
            }
            if matches!(
                commands.try_recv(),
                Ok(Command::Shutdown) | Err(mpsc::TryRecvError::Disconnected)
            ) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "shutdown before initial frame",
                ));
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "initial paint frame was not provided",
                ));
            }
            thread::sleep(Duration::from_millis(3));
        }
    }

    fn initialize(&mut self, frame: Frame) -> io::Result<()> {
        validate_frame(&frame, self.layout)?;
        let context_id = self.session.info().root_context_id;
        let surface = self.session.create_surface(
            surface_definition(
                context_id,
                self.session.allocate_id()?,
                frame.width,
                frame.height,
            ),
            &RequestMetadata::default(),
        )?;
        let node = terminal_node(&mut self.session, &surface, self.layout)?;
        self.session
            .create_node(&node, &RequestMetadata::default())?;
        let raster = create_and_prime(&mut self.session, &surface, &frame)?;
        activate(&mut self.session, &surface, &raster)?;
        self.last_pixels = Some(frame.pixels);
        self.surface = Some(surface);
        self.node = Some(node);
        self.raster = Some(raster);
        self.force_full = false;
        Ok(())
    }

    fn submit(&mut self, frame: Frame) -> io::Result<()> {
        validate_frame_pixels(&frame)?;
        if frame.width != self.layout.backing_width || frame.height != self.layout.backing_height {
            // A complete old-size frame may already be in the replaceable mailbox when a
            // settled TARGET_CHANGED is applied. It is authoritative for the old backing, but
            // must not fail the control session or replace the newly requested backing.
            return Ok(());
        }
        let resize = self
            .raster
            .as_ref()
            .is_some_and(|raster| raster.width != frame.width || raster.height != frame.height);
        if resize {
            self.replace_raster(frame)?;
            return Ok(());
        }
        let previous = self
            .last_pixels
            .as_deref()
            .ok_or_else(|| io::Error::other("missing authoritative frame"))?;
        let damage = damage_rect(previous, &frame.pixels, frame.width, frame.height);
        if damage.is_none() && !self.force_full {
            return Ok(());
        }
        let raster = self
            .raster
            .as_mut()
            .ok_or_else(|| io::Error::other("missing raster track"))?;
        raster.frame_id = raster
            .frame_id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("raster frame ID exhausted"))?;
        let full = self.force_full
            || !raster.delta
            || damage.is_none_or(|rect| {
                rect.area() * 2 >= u64::from(frame.width) * u64::from(frame.height)
            });
        if full {
            send_full(raster, &frame.pixels)?;
        } else if let Some(rect) = damage {
            let pixels = extract_rect(&frame.pixels, frame.width, rect)?;
            let operation = RasterDeltaOperation::Overwrite {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
                rgba: &pixels,
            };
            if raster.zstd {
                raster.channel.send_raster_delta_adaptive(
                    1,
                    raster.frame_id,
                    raster.frame_id - 1,
                    i64::try_from(raster.frame_id.saturating_mul(1_000_000) / FRAME_RATE)
                        .unwrap_or(i64::MAX),
                    1_000_000 / FRAME_RATE,
                    &[operation],
                )?;
            } else {
                raster.channel.send_raster_delta(
                    1,
                    raster.frame_id,
                    raster.frame_id - 1,
                    i64::try_from(raster.frame_id.saturating_mul(1_000_000) / FRAME_RATE)
                        .unwrap_or(i64::MAX),
                    1_000_000 / FRAME_RATE,
                    &[operation],
                    false,
                )?;
            }
        }
        self.last_pixels = Some(frame.pixels);
        self.force_full = false;
        Ok(())
    }

    fn replace_raster(&mut self, frame: Frame) -> io::Result<()> {
        let surface = self
            .surface
            .as_ref()
            .ok_or_else(|| io::Error::other("missing surface"))?;
        let mut replacement = surface.definition()?;
        replacement.logical_width = u64::from(frame.width);
        replacement.logical_height = u64::from(frame.height);
        self.session
            .update_surface(surface, replacement, &RequestMetadata::default())?;
        let next = create_and_prime(&mut self.session, surface, &frame)?;
        activate(&mut self.session, surface, &next)?;
        if let Some(old) = self.raster.replace(next) {
            let _ = old.channel.eos();
            destroy_if_live(&mut self.session, &old.track)?;
            drop(old.channel);
        }
        self.last_pixels = Some(frame.pixels);
        self.force_full = false;
        Ok(())
    }

    fn poll_session(&mut self) -> io::Result<()> {
        while let Some(event) = self.session.take_event()? {
            match event {
                SessionEvent::TargetChanged(payload) => {
                    self.session.apply_target_changed(&payload)?;
                    if !self.session.info().target_settled()? {
                        continue;
                    }
                    let layout = layout(&self.session, self.resolution_scale)?;
                    if layout != self.layout {
                        self.layout = layout;
                        if let Some(node) = &mut self.node {
                            update_node(node, layout)?;
                            self.session
                                .update_node(node, &RequestMetadata::default())?;
                        }
                        let _ = self.events.send(Event::Resized(layout));
                    }
                }
                SessionEvent::TrackLost { object_id, .. }
                    if self
                        .raster
                        .as_ref()
                        .is_some_and(|raster| raster.track.id() == object_id) =>
                {
                    self.recover_track()?;
                }
                SessionEvent::ConnectionClosed { diagnostic } => {
                    return Err(io::Error::new(io::ErrorKind::BrokenPipe, diagnostic));
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn poll_channel(&mut self) -> io::Result<()> {
        let mut recover = false;
        let mut need_full = false;
        if let Some(raster) = &self.raster {
            while let Some(event) = raster.channel.take_event()? {
                match event {
                    ChannelEvent::NeedFullFrame(_) => need_full = true,
                    ChannelEvent::Error(_) | ChannelEvent::NeedKeyframe(_) => recover = true,
                }
            }
        }
        if recover {
            self.recover_track()?;
        } else if need_full {
            self.force_full = true;
            if let (Some(pixels), Some(raster)) = (self.last_pixels.as_ref(), self.raster.as_mut())
            {
                raster.frame_id = raster
                    .frame_id
                    .checked_add(1)
                    .ok_or_else(|| io::Error::other("raster frame ID exhausted"))?;
                send_full(raster, pixels)?;
                self.force_full = false;
            }
        }
        Ok(())
    }

    fn recover_track(&mut self) -> io::Result<()> {
        let (width, height) = self
            .raster
            .as_ref()
            .map(|raster| (raster.width, raster.height))
            .ok_or_else(|| io::Error::other("cannot recover a missing raster track"))?;
        let pixels = self
            .last_pixels
            .clone()
            .ok_or_else(|| io::Error::other("cannot recover without an authoritative frame"))?;
        let frame = Frame {
            width,
            height,
            pixels,
        };
        let surface = self
            .surface
            .as_ref()
            .ok_or_else(|| io::Error::other("missing surface"))?;
        let next = create_and_prime(&mut self.session, surface, &frame)?;
        activate(&mut self.session, surface, &next)?;
        if let Some(old) = self.raster.replace(next) {
            let _ = destroy_if_live(&mut self.session, &old.track);
            drop(old.channel);
        }
        self.force_full = false;
        Ok(())
    }

    fn teardown(mut self) -> io::Result<()> {
        let mut first = None;
        if let Some(raster) = self.raster.take() {
            let _ = raster.channel.eos();
            if let Some(node) = self.node.take()
                && let Err(error) = self.session.delete_node(
                    node.owning_context_id,
                    node.node_id,
                    &RequestMetadata::default(),
                )
            {
                first = Some(error);
            }
            if let Err(error) = destroy_if_live(&mut self.session, &raster.track)
                && first.is_none()
            {
                first = Some(error);
            }
            drop(raster.channel);
        }
        if let Some(surface) = self.surface.take()
            && let Err(error) = self
                .session
                .destroy_surface(&surface, &RequestMetadata::default())
            && first.is_none()
        {
            first = Some(error);
        }
        if let Err(error) = self.session.close()
            && first.is_none()
        {
            first = Some(error);
        }
        first.map_or(Ok(()), Err)
    }
}

fn take_latest(latest: &Arc<Mutex<Option<Frame>>>) -> io::Result<Option<Frame>> {
    Ok(latest
        .lock()
        .map_err(|_| io::Error::other("frame mailbox is poisoned"))?
        .take())
}

fn wait_for_layout(session: &mut Session, scale: f32) -> io::Result<Layout> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match layout(session, scale) {
            Ok(layout) => return Ok(layout),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
        while let Some(event) = session.take_event()? {
            match event {
                SessionEvent::TargetChanged(payload) => {
                    session.apply_target_changed(&payload)?;
                }
                SessionEvent::ConnectionClosed { diagnostic } => {
                    return Err(io::Error::new(io::ErrorKind::BrokenPipe, diagnostic));
                }
                _ => {}
            }
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "terminal target geometry did not settle",
            ));
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn layout(session: &Session, scale: f32) -> io::Result<Layout> {
    let descriptor = &session.info().target_descriptor;
    let value = |key| {
        descriptor
            .iter()
            .find(|(candidate, _)| *candidate == key)
            .and_then(|(_, value)| value.as_u64())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "terminal descriptor is incomplete",
                )
            })
    };
    let viewport_width =
        u32::try_from(value(0)?).map_err(|_| io::Error::other("viewport width exceeds u32"))?;
    let viewport_height =
        u32::try_from(value(1)?).map_err(|_| io::Error::other("viewport height exceeds u32"))?;
    let columns =
        u32::try_from(value(2)?).map_err(|_| io::Error::other("column count exceeds u32"))?;
    let rows = u32::try_from(value(3)?).map_err(|_| io::Error::other("row count exceeds u32"))?;
    let cell_width =
        u32::try_from(value(4)?).map_err(|_| io::Error::other("cell width exceeds u32"))?;
    let cell_height =
        u32::try_from(value(5)?).map_err(|_| io::Error::other("cell height exceeds u32"))?;
    let settled = descriptor
        .iter()
        .find(|(key, _)| *key == 6)
        .and_then(|(_, value)| value.as_bool())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "terminal settled flag is missing",
            )
        })?;
    if !settled {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "terminal target is not settled",
        ));
    }
    if columns == 0 || rows == 0 || cell_width == 0 || cell_height == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "terminal target has zero geometry",
        ));
    }
    let canvas_rows = rows
        .saturating_sub(1)
        .min(rows.saturating_sub(RESERVED_UI_ROWS))
        .max(1);
    let grid_width = columns
        .checked_mul(cell_width)
        .ok_or_else(|| io::Error::other("grid width overflow"))?
        .min(viewport_width);
    let grid_height = rows
        .checked_mul(cell_height)
        .ok_or_else(|| io::Error::other("grid height overflow"))?
        .min(viewport_height);
    let physical_width = grid_width;
    let physical_height = canvas_rows
        .checked_mul(cell_height)
        .ok_or_else(|| io::Error::other("canvas height overflow"))?
        .min(grid_height);
    let desired_width = (physical_width as f64 * f64::from(scale)).round().max(1.0) as u32;
    let desired_height = (physical_height as f64 * f64::from(scale)).round().max(1.0) as u32;
    let contract = &session.info().resource_contract;
    let pixel_limit = contract
        .get(Resource::CodedPixelsPerTrack)
        .min(contract.get(Resource::RetainedPixels));
    let body_limit = contract.get(Resource::MediaRecordBody);
    let (backing_width, backing_height) =
        bounded_backing(desired_width, desired_height, body_limit, pixel_limit)?;
    Ok(Layout {
        columns,
        rows,
        canvas_rows,
        viewport_width,
        viewport_height,
        grid_origin_x: viewport_width.saturating_sub(grid_width) / 2,
        grid_origin_y: viewport_height.saturating_sub(grid_height) / 2,
        cell_width,
        cell_height,
        backing_width,
        backing_height,
    })
}

fn bounded_backing(
    width: u32,
    height: u32,
    body_limit: u64,
    pixel_limit: u64,
) -> io::Result<(u32, u32)> {
    if body_limit == 0 || pixel_limit == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "presenter offered no raster capacity",
        ));
    }
    let pixels = u64::from(width) * u64::from(height);
    let body_pixels = body_limit.saturating_sub(64) / 4;
    let allowed = pixel_limit.min(body_pixels).max(1);
    let scale = (allowed as f64 / pixels.max(1) as f64)
        .sqrt()
        .min(8192.0 / width.max(1) as f64)
        .min(8192.0 / height.max(1) as f64)
        .min(1.0);
    let mut output = (
        (width as f64 * scale).floor().max(1.0) as u32,
        (height as f64 * scale).floor().max(1.0) as u32,
    );
    loop {
        let body = media::rgba8_raw_frame_body_len(output.0, output.1)
            .map(u64::from)
            .unwrap_or(u64::MAX);
        if u64::from(output.0) * u64::from(output.1) <= pixel_limit && body <= body_limit {
            return Ok(output);
        }
        if output.0 >= output.1 && output.0 > 1 {
            output.0 -= 1;
        } else if output.1 > 1 {
            output.1 -= 1;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "presenter cannot hold one RGBA pixel",
            ));
        }
    }
}

fn surface_definition(
    context_id: u64,
    surface_id: u64,
    width: u32,
    height: u32,
) -> SurfaceDefinition {
    SurfaceDefinition {
        context_id,
        surface_id,
        semantic_profile: vivid_sdk::GENERIC_CONTENT.into(),
        coordinate_model: CoordinateModel::DesktopLogicalPixels,
        logical_width: u64::from(width),
        logical_height: u64::from(height),
        scale_numerator: 1,
        scale_denominator: 1,
        rotation: 0,
        descriptor: SurfaceDescriptor {
            role: SurfaceRole::Document,
            title: "vvpaint".into(),
            semantic_content_revision: 1,
            semantic_availability: 0,
            locator_hint: String::new(),
        },
        policy: 0,
        profile_parameters: vec![],
    }
}

fn terminal_node(
    session: &mut Session,
    surface: &Surface,
    layout: Layout,
) -> io::Result<SceneNode> {
    let mut node = SceneNode {
        owning_context_id: surface.context_id(),
        node_id: session.allocate_id()?,
        surface_context_id: surface.context_id(),
        surface_id: surface.id(),
        geometry: vec![],
        fit: Fit::Fill,
        linear_sampling: true,
        z_index: 0,
        visible: true,
        opacity: u16::MAX,
        clip: None,
    };
    update_node(&mut node, layout)?;
    Ok(node)
}

fn update_node(node: &mut SceneNode, layout: Layout) -> io::Result<()> {
    let width = i64::from(layout.columns)
        .checked_shl(32)
        .ok_or_else(|| io::Error::other("node width overflow"))?;
    let height = i64::from(layout.canvas_rows)
        .checked_shl(32)
        .ok_or_else(|| io::Error::other("node height overflow"))?;
    node.geometry = vec![
        (0, Value::Unsigned(1)),
        (1, Value::Unsigned(0)),
        (2, Value::Unsigned(0)),
        (3, Value::Unsigned(width as u64)),
        (4, Value::Unsigned(height as u64)),
        (5, Value::Unsigned(1)),
    ];
    Ok(())
}

fn create_and_prime(
    session: &mut Session,
    surface: &Surface,
    frame: &Frame,
) -> io::Result<RasterState> {
    let track_id = session.allocate_id()?;
    for (delta, zstd) in [(true, true), (false, true), (true, false), (false, false)] {
        let configuration =
            raster_configuration(surface, track_id, frame.width, frame.height, delta, zstd)?;
        let mut probe = configuration.clone();
        probe.track_id = 0;
        if !session.probe_track(&probe)?.supported {
            continue;
        }
        let track = session.create_track(configuration, &RequestMetadata::default())?;
        let result = (|| {
            let channel = session.open_track_channel(&track)?;
            if zstd {
                channel.send_raster_adaptive(1, 1, &frame.pixels)?;
            } else {
                channel.send_raster(1, 1, &frame.pixels, false)?;
            }
            session.wait_track(
                &track,
                TrackWaitCondition::MilestoneSet,
                Some(MILESTONE_OUTPUT_READY),
                READY_TIMEOUT_US,
            )?;
            Ok::<_, io::Error>(RasterState {
                track: track.clone(),
                channel,
                width: frame.width,
                height: frame.height,
                frame_id: 1,
                delta,
                zstd,
            })
        })();
        match result {
            Ok(value) => return Ok(value),
            Err(error) => {
                let _ = session.destroy_track(&track, &RequestMetadata::default());
                return Err(error);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "presenter rejected every vvpaint raster configuration",
    ))
}

fn raster_configuration(
    surface: &Surface,
    track_id: u64,
    width: u32,
    height: u32,
    delta: bool,
    zstd: bool,
) -> io::Result<TrackConfiguration> {
    let body = media::rgba8_raw_frame_body_len(width, height).map_err(io::Error::other)?;
    Ok(TrackConfiguration {
        context_id: surface.context_id(),
        surface_id: surface.id(),
        track_id,
        slot: SLOT_RASTER,
        mode: TrackMode::Live,
        lane: LaneClass::Bulk,
        maximum_record_body: body,
        maximum_rate_millihertz: FRAME_RATE * 1000,
        maximum_encoded_bits_per_second: u64::from(body)
            .saturating_mul(8)
            .saturating_mul(FRAME_RATE),
        maximum_records_per_second: FRAME_RATE,
        maximum_inflight_body_bytes: u64::from(body).saturating_mul(2),
        kind: KindConfiguration::Raster(RasterConfiguration {
            width,
            height,
            alpha_mode: 1,
            delta_enabled: delta,
            maximum_delta_operations: 1,
            zstd_enabled: zstd,
        }),
        target_latency_us: 33_333,
        maximum_latency_us: 250_000,
        retained_pixel_charge: u64::from(width) * u64::from(height),
    })
}

fn activate(session: &mut Session, surface: &Surface, raster: &RasterState) -> io::Result<()> {
    session
        .activate_tracks(
            surface,
            &[SlotBinding {
                slot: SLOT_RASTER,
                track_id: raster.track.id(),
                expected_channel_generation: raster.track.channel_generation(),
                required_milestone: MILESTONE_OUTPUT_READY,
            }],
            &RequestMetadata::default(),
        )
        .map(|_| ())
}

fn send_full(raster: &RasterState, pixels: &[u8]) -> io::Result<()> {
    if raster.zstd {
        raster
            .channel
            .send_raster_adaptive(1, raster.frame_id, pixels)
            .map(|_| ())
    } else {
        raster
            .channel
            .send_raster(1, raster.frame_id, pixels, false)
            .map(|_| ())
    }
}

fn destroy_if_live(session: &mut Session, track: &Track) -> io::Result<()> {
    match session.destroy_track(track, &RequestMetadata::default()) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[derive(Clone, Copy)]
struct Rect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}
impl Rect {
    fn area(self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }
}

fn damage_rect(previous: &[u8], current: &[u8], width: u32, height: u32) -> Option<Rect> {
    if previous.len() != current.len() {
        return Some(Rect {
            x: 0,
            y: 0,
            width,
            height,
        });
    }
    let mut bounds = (width, height, 0, 0);
    let mut changed = false;
    for (index, (before, after)) in previous
        .chunks_exact(4)
        .zip(current.chunks_exact(4))
        .enumerate()
    {
        if before != after {
            changed = true;
            let x = index as u32 % width;
            let y = index as u32 / width;
            bounds.0 = bounds.0.min(x);
            bounds.1 = bounds.1.min(y);
            bounds.2 = bounds.2.max(x);
            bounds.3 = bounds.3.max(y);
        }
    }
    if changed {
        Some(Rect {
            x: bounds.0,
            y: bounds.1,
            width: bounds.2 - bounds.0 + 1,
            height: bounds.3 - bounds.1 + 1,
        })
    } else {
        None
    }
}

fn extract_rect(pixels: &[u8], frame_width: u32, rect: Rect) -> io::Result<Vec<u8>> {
    let row_bytes = usize::try_from(u64::from(rect.width) * 4).map_err(io::Error::other)?;
    let mut output = Vec::with_capacity(row_bytes * rect.height as usize);
    for y in rect.y..rect.y + rect.height {
        let start =
            usize::try_from((u64::from(y) * u64::from(frame_width) + u64::from(rect.x)) * 4)
                .map_err(io::Error::other)?;
        output.extend_from_slice(&pixels[start..start + row_bytes]);
    }
    Ok(output)
}

fn validate_frame(frame: &Frame, layout: Layout) -> io::Result<()> {
    validate_frame_pixels(frame)?;
    if frame.width != layout.backing_width || frame.height != layout.backing_height {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "paint frame does not match settled Vivid layout",
        ));
    }
    Ok(())
}

fn validate_frame_pixels(frame: &Frame) -> io::Result<()> {
    let expected = usize::try_from(u64::from(frame.width) * u64::from(frame.height) * 4)
        .map_err(io::Error::other)?;
    if frame.pixels.len() != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "paint frame has an invalid RGBA byte length",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vivid_protocol::auth::Secret32;
    use vivid_sdk::testing::{ROOT_SECRET_HEX, TestPresenter};

    #[test]
    fn damage_bounds_and_extract_are_exact() {
        let before = vec![0; 4 * 4 * 4];
        let mut after = before.clone();
        for index in [5usize, 6, 9, 10] {
            after[index * 4] = 1;
        }
        let rect = damage_rect(&before, &after, 4, 4).unwrap();
        assert_eq!((rect.x, rect.y, rect.width, rect.height), (1, 1, 2, 2));
        assert_eq!(extract_rect(&after, 4, rect).unwrap().len(), 16);
    }

    #[test]
    fn negotiates_and_presents_through_shared_test_presenter() {
        let presenter = TestPresenter::start(20, 10).unwrap();
        let secret = Secret32::from_hex(ROOT_SECRET_HEX).unwrap();
        let mut config = producer_config();
        config.endpoint_control = Some(presenter.endpoint().to_owned());
        config.endpoint_bulk = Some(presenter.endpoint().to_owned());
        config.authentication = ProducerAuthentication::Root {
            root_secret: secret,
        };
        let handle = VividHandle::spawn(config, 0.25).unwrap();
        let layout = handle.wait_ready().unwrap();
        let blank = RgbaImage::new(layout.backing_width, layout.backing_height);
        handle.publish(blank.clone()).unwrap();
        handle.wait_presented().unwrap();

        // A duplicate authoritative frame is a no-op and must not consume a media frame ID. The
        // next changed frame still bases its delta on the actual preceding record accepted by the
        // channel, not on the duplicate UI publication.
        handle.publish(blank.clone()).unwrap();
        thread::sleep(Duration::from_millis(75));
        let mut changed = blank;
        changed.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        handle.publish(changed).unwrap();
        thread::sleep(Duration::from_millis(75));
        if let Some(Event::Error(error)) = handle.try_event() {
            panic!("duplicate frame corrupted raster sequencing: {error}");
        }

        presenter.resize_terminal(24, 12, true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let resized = loop {
            match handle.try_event() {
                Some(Event::Resized(layout)) => break layout,
                Some(Event::Error(error)) => panic!("resize failed: {error}"),
                _ if Instant::now() >= deadline => panic!("resize was not delivered"),
                _ => thread::sleep(Duration::from_millis(5)),
            }
        };
        handle
            .publish(RgbaImage::new(
                resized.backing_width,
                resized.backing_height,
            ))
            .unwrap();
        wait_for_observed(&presenter, vivid_protocol::messages::CREATE_TRACK, 2);

        let active_track = presenter
            .observed()
            .into_iter()
            .rfind(|value| value.record_type == vivid_protocol::messages::CREATE_TRACK)
            .unwrap();
        let value = |key| {
            active_track
                .payload
                .iter()
                .find(|(candidate, _)| *candidate == key)
                .and_then(|(_, value)| value.as_u64())
                .unwrap()
        };
        presenter
            .lose_track(value(0), value(1), active_track.object_id)
            .unwrap();
        wait_for_observed(&presenter, vivid_protocol::messages::CREATE_TRACK, 3);

        handle.shutdown();
        let observed = presenter.observed();
        assert!(
            observed
                .iter()
                .any(|value| value.record_type == vivid_protocol::messages::CREATE_SURFACE)
        );
        assert!(
            observed
                .iter()
                .any(|value| value.record_type == vivid_protocol::messages::ACTIVATE_TRACK)
        );
    }

    fn wait_for_observed(presenter: &TestPresenter, record_type: u16, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while presenter
            .observed()
            .iter()
            .filter(|value| value.record_type == record_type)
            .count()
            < count
        {
            assert!(
                Instant::now() < deadline,
                "record {record_type:#x} timed out"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
}
