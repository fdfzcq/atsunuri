//! Pen input through the Wayland tablet protocol (`zwp_tablet_v2`).
//!
//! Tool events are buffered until the protocol's `frame` event and then handed
//! to [`App::tool_frame`] as one [`ToolFrame`]. Tablets and pads are tracked
//! only so their objects are handled correctly; their events are not used yet.

use std::sync::Mutex;

use smithay_client_toolkit::dispatch2::Dispatch2;
use wayland_client::{Connection, Proxy, QueueHandle, WEnum, event_created_child};
use wayland_protocols::wp::tablet::zv2::client::{
    zwp_tablet_manager_v2::ZwpTabletManagerV2,
    zwp_tablet_pad_group_v2::{self, ZwpTabletPadGroupV2},
    zwp_tablet_pad_ring_v2::ZwpTabletPadRingV2,
    zwp_tablet_pad_strip_v2::ZwpTabletPadStripV2,
    zwp_tablet_pad_v2::{self, ZwpTabletPadV2},
    zwp_tablet_seat_v2::{self, ZwpTabletSeatV2},
    zwp_tablet_tool_v2::{self, ZwpTabletToolV2},
    zwp_tablet_v2::{self, ZwpTabletV2},
};

use crate::app::App;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolKind {
    #[default]
    Pen,
    Eraser,
    Other,
}

/// Everything a tool reported in one protocol frame, plus its current state.
#[derive(Debug, Clone, Default)]
pub struct ToolFrame {
    pub kind: ToolKind,
    /// Surface-local position in logical pixels.
    pub position: (f64, f64),
    /// Normalized pressure in `[0, 1]`.
    pub pressure: f64,
    pub proximity_in: bool,
    pub proximity_out: bool,
    /// The tip touched the surface in this frame.
    pub down: bool,
    /// The tip left the surface in this frame.
    pub up: bool,
    pub moved: bool,
}

pub struct ManagerData;
pub struct SeatData;
pub struct TabletData;
pub struct PadData;
pub struct PadGroupData;
pub struct PadPartData;

#[derive(Default)]
pub struct ToolData(Mutex<ToolFrame>);

impl Dispatch2<ZwpTabletManagerV2, App> for ManagerData {
    fn event(
        &self,
        _: &mut App,
        _: &ZwpTabletManagerV2,
        _: <ZwpTabletManagerV2 as Proxy>::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
    }
}

impl Dispatch2<ZwpTabletSeatV2, App> for SeatData {
    event_created_child!(App, ZwpTabletSeatV2, [
        zwp_tablet_seat_v2::EVT_TABLET_ADDED_OPCODE => (ZwpTabletV2, TabletData),
        zwp_tablet_seat_v2::EVT_TOOL_ADDED_OPCODE => (ZwpTabletToolV2, ToolData::default()),
        zwp_tablet_seat_v2::EVT_PAD_ADDED_OPCODE => (ZwpTabletPadV2, PadData),
    ]);

    fn event(
        &self,
        _: &mut App,
        _: &ZwpTabletSeatV2,
        event: zwp_tablet_seat_v2::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
        if let zwp_tablet_seat_v2::Event::TabletAdded { id } = event {
            log::info!("tablet added: {:?}", id.id());
        }
    }
}

impl Dispatch2<ZwpTabletV2, App> for TabletData {
    fn event(
        &self,
        _: &mut App,
        tablet: &ZwpTabletV2,
        event: zwp_tablet_v2::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
        match event {
            zwp_tablet_v2::Event::Name { name } => log::info!("tablet name: {name}"),
            zwp_tablet_v2::Event::Removed => tablet.destroy(),
            _ => {}
        }
    }
}

impl Dispatch2<ZwpTabletToolV2, App> for ToolData {
    fn event(
        &self,
        app: &mut App,
        tool: &ZwpTabletToolV2,
        event: zwp_tablet_tool_v2::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
        use zwp_tablet_tool_v2::{Event, Type};

        let mut state = self.0.lock().unwrap();
        match event {
            Event::Type { tool_type } => {
                state.kind = match tool_type {
                    WEnum::Value(Type::Pen | Type::Brush | Type::Pencil | Type::Airbrush) => {
                        ToolKind::Pen
                    }
                    WEnum::Value(Type::Eraser) => ToolKind::Eraser,
                    _ => ToolKind::Other,
                };
            }
            Event::ProximityIn { .. } => state.proximity_in = true,
            Event::ProximityOut => state.proximity_out = true,
            Event::Down { .. } => state.down = true,
            Event::Up => state.up = true,
            Event::Motion { x, y } => {
                state.position = (x, y);
                state.moved = true;
            }
            Event::Pressure { pressure } => state.pressure = pressure as f64 / 65535.0,
            Event::Frame { .. } => {
                let frame = state.clone();
                state.proximity_in = false;
                state.proximity_out = false;
                state.down = false;
                state.up = false;
                state.moved = false;
                drop(state);
                app.tool_frame(&frame);
            }
            Event::Removed => tool.destroy(),
            _ => {}
        }
    }
}

impl Dispatch2<ZwpTabletPadV2, App> for PadData {
    event_created_child!(App, ZwpTabletPadV2, [
        zwp_tablet_pad_v2::EVT_GROUP_OPCODE => (ZwpTabletPadGroupV2, PadGroupData),
    ]);

    fn event(
        &self,
        _: &mut App,
        pad: &ZwpTabletPadV2,
        event: zwp_tablet_pad_v2::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
        if let zwp_tablet_pad_v2::Event::Removed = event {
            pad.destroy();
        }
    }
}

impl Dispatch2<ZwpTabletPadGroupV2, App> for PadGroupData {
    event_created_child!(App, ZwpTabletPadGroupV2, [
        zwp_tablet_pad_group_v2::EVT_RING_OPCODE => (ZwpTabletPadRingV2, PadPartData),
        zwp_tablet_pad_group_v2::EVT_STRIP_OPCODE => (ZwpTabletPadStripV2, PadPartData),
    ]);

    fn event(
        &self,
        _: &mut App,
        _: &ZwpTabletPadGroupV2,
        _: zwp_tablet_pad_group_v2::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
    }
}

impl Dispatch2<ZwpTabletPadRingV2, App> for PadPartData {
    fn event(
        &self,
        _: &mut App,
        _: &ZwpTabletPadRingV2,
        _: <ZwpTabletPadRingV2 as Proxy>::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
    }
}

impl Dispatch2<ZwpTabletPadStripV2, App> for PadPartData {
    fn event(
        &self,
        _: &mut App,
        _: &ZwpTabletPadStripV2,
        _: <ZwpTabletPadStripV2 as Proxy>::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
    }
}
