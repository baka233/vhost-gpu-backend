// virtio-gpu protocol structure and const
use std::marker::PhantomData;

use std::fmt;
use std::fmt::Formatter;
use std::str::from_utf8;
use std::cmp::min;

use ::vm_memory::{ Le32, Le64, GuestAddress, ByteValued, Bytes, GuestMemoryError, GuestMemoryMmap };
use std::mem::{size_of_val, size_of};
use vm_memory::guest_memory::Error;
use crate::protocol::VirtioGpuCommandDecodeError::ParserError;
use std::num::TryFromIntError;
use rutabaga_gfx::RutabagaError;


// virtio-gpu protocol based on
// https://docs.oasis-open.org/virtio/virtio/v1.1/cs01/virtio-v1.1-cs01.html#x1-3200007 and
// https://elixir.bootlin.com/linux/latest/source/include/uapi/linux/virtio_gpu.h#L62 (linux uapi header)
/* 2D commands */
pub const VIRTIO_GPU_CMD_GET_DISPLAY_INFO: u32           = 0x0100;
pub const VIRTIO_GPU_CMD_RESOURCE_CREATE_2D: u32         = 0x0101;
pub const VIRTIO_GPU_CMD_RESOURCE_UNREF: u32             = 0x0102;
pub const VIRTIO_GPU_CMD_SET_SCANOUT: u32                = 0x0103;
pub const VIRTIO_GPU_CMD_RESOURCE_FLUSH: u32             = 0x0104;
pub const VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D: u32        = 0x0105;
pub const VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING: u32    = 0x0106;
pub const VIRTIO_GPU_CMD_RESOURCE_DETACH_BACKING: u32    = 0x0107;
pub const VIRTIO_GPU_CMD_GET_CAPSET_INFO: u32            = 0x0108;
pub const VIRTIO_GPU_CMD_GET_CAPSET: u32                 = 0x0109;
pub const VIRTIO_GPU_CMD_GET_EDID: u32                   = 0x010a;

// 3D command based on qemu virtio_gpu
// https://github.com/qemu/qemu/blob/master/include/standard-headers/linux/virtio_gpu.h
/* 3D commands */
pub const VIRTIO_GPU_CMD_CTX_CREATE: u32                = 0x0200;
pub const VIRTIO_GPU_CMD_CTX_DESTROY: u32               = 0x0201;
pub const VIRTIO_GPU_CMD_CTX_ATTACH_RESOURCE: u32       = 0x0202;
pub const VIRTIO_GPU_CMD_CTX_DETACH_RESOURCE: u32       = 0x0203;
pub const VIRTIO_GPU_CMD_RESOURCE_CREATE_3D: u32        = 0x0204;
pub const VIRTIO_GPU_CMD_TRANSFER_TO_HOST_3D: u32       = 0x0205;
pub const VIRTIO_GPU_CMD_TRANSFER_FROM_HOST_3D: u32     = 0x0206;
pub const VIRTIO_GPU_CMD_SUBMIT_3D: u32                 = 0x0207;

/* cursor commands */
pub const VIRTIO_GPU_CMD_UPDATE_CURSOR: u32             = 0x0301;
pub const VIRTIO_GPU_CMD_MOVE_CURSOR: u32               = 0x0302;


/* success responses */
pub const VIRTIO_GPU_RESP_OK_NODATA: u32                = 0x1100;
pub const VIRTIO_GPU_RESP_OK_DISPLAY_INFO: u32          = 0x1101;
pub const VIRTIO_GPU_RESP_OK_CAPSET_INFO: u32           = 0x1102;
pub const VIRTIO_GPU_RESP_OK_CAPSET: u32                = 0x1103;
pub const VIRTIO_GPU_RESP_OK_EDID: u32                  = 0x1104;
pub const VIRTIO_GPU_RESP_OK_RESOURCE_UUID: u32         = 0x1105;

/* error responses */
pub const VIRTIO_GPU_RESP_ERR_UNSPEC: u32               = 0x1200;
pub const VIRTIO_GPU_RESP_ERR_OUT_OF_MEMORY: u32        = 0x1201;
pub const VIRTIO_GPU_RESP_ERR_INVALID_SCANOUT_ID: u32   = 0x1202;
pub const VIRTIO_GPU_RESP_ERR_INVALID_RESOURCE_ID: u32  = 0x1203;
pub const VIRTIO_GPU_RESP_ERR_INVALID_CONTEXT_ID: u32   = 0x1204;
pub const VIRTIO_GPU_RESP_ERR_INVALID_PARAMETER: u32    = 0x1205;

pub const VIRTIO_GPU_FLAG_FENCE: u32 = 1 << 0;
/* Fence context index info flag not upstreamed. */
pub const VIRTIO_GPU_FLAG_INFO_FENCE_CTX_IDX: u32 = 1 << 1;


// Device type
pub const VIRTIO_GPU_DEVICE_TYPE: u32 = 16;


//----- feature flags ----
pub const VIRTIO_GPU_F_VIRGL: u32         = 0;
pub const VIRTIO_GPU_F_EDID: u32          = 1;
pub const VIRTIO_GPU_F_RESOURCE_UUID: u32 = 2;

//----- virtio-gpu control header and command header ----
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_ctrl_hdr {
    pub type_:    Le32,
    pub flags:    Le32,
    pub fence_id: Le64,
    pub ctx_id:   Le32,
    pub padding:  Le32,
}

unsafe impl ByteValued for virtio_gpu_ctrl_hdr{}

/* data passed in the cursor wq */
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_cursor_pos {
    pub scanout_id: Le32,
    pub x:          Le32,
    pub y:          Le32,
    pub padding:    Le32,
}

unsafe impl ByteValued for virtio_gpu_cursor_pos{}

/* VIRTIO_GPU_CMD_UPDATE_CURSOR, VIRTIO_GPU_CMD_MOVE_CURSOR */
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_update_cursor {
    pub hdr:           virtio_gpu_ctrl_hdr,
    pub pos:           virtio_gpu_cursor_pos, /* update & move */
    pub resource_id:   Le32,                  /* update only */
    pub hot_x:         Le32,                  /* update only */
    pub padding:       Le32,
}

unsafe impl ByteValued for virtio_gpu_update_cursor{}


/* data passed in the control wq, 2d related */
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_rect {
    pub x:          Le32,
    pub y:          Le32,
    pub width:      Le32,
    pub height:     Le32,
}

unsafe impl ByteValued for virtio_gpu_rect{}

/* VIRTIO_GPU_CMD_RESOURCE_UNREF */
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_resource_unref {
    pub hdr:            virtio_gpu_ctrl_hdr,
    pub resource_id:    Le32,
    pub padding:        Le32,
}

unsafe impl ByteValued for virtio_gpu_resource_unref {}

/* VIRTIO_GPU_CMD_RESOURCE_CREATE_2D: create a 2d resource with a format */
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_resource_create_2d {
    pub hdr:         virtio_gpu_ctrl_hdr,
    pub resource_id: Le32,
    pub format:      Le32,
    pub width:       Le32,
    pub height:      Le32,
}

unsafe impl ByteValued for virtio_gpu_resource_create_2d{}

/* VIRTIO_GPU_CMD_SET_SCANOUT */
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_set_scanout {
    pub hdr:            virtio_gpu_ctrl_hdr,
    pub r:              virtio_gpu_rect,
    pub scanout_id:     Le32,
    pub resource_id:    Le32,
}

unsafe impl ByteValued for virtio_gpu_set_scanout{}

/* VIRTIO_GPU_CMD_RESOURCE_FLUSH */
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_resource_flush {
    pub hdr:         virtio_gpu_ctrl_hdr,
    pub r:           virtio_gpu_rect,
    pub resource_id: Le32,
    pub padding:     Le32,
}

unsafe impl ByteValued for virtio_gpu_resource_flush{}

/* VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D: simple transfer to_host */
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_transfer_to_host_2d {
    pub hdr:            virtio_gpu_ctrl_hdr,
    pub r:              virtio_gpu_rect,
    pub offset:         Le64,
    pub resource_id:    Le32,
    pub padding:        Le32,
}

unsafe impl ByteValued for virtio_gpu_transfer_to_host_2d{}

#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_mem_entry {
    pub addr:       Le64,
    pub length:     Le32,
    pub padding:    Le32,
}

unsafe impl ByteValued for virtio_gpu_mem_entry{}

/* VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING */
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_resource_attach_backing {
    pub hdr:           virtio_gpu_ctrl_hdr,
    pub resource_id:   Le32,
    pub nr_entries:    Le32,
}

unsafe impl ByteValued for virtio_gpu_resource_attach_backing{}

/* VIRTIO_GPU_CMD_RESOURCE_DETACH_BACKING */
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_resource_detach_backing {
    pub hdr:            virtio_gpu_ctrl_hdr,
    pub resource_id:    Le32,
    pub padding:        Le32,
}

unsafe impl ByteValued for virtio_gpu_resource_detach_backing{}

#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_display_one {
    pub r:        virtio_gpu_rect,
    pub enabled:  Le32,
    pub flags:    Le32,
}

unsafe impl ByteValued for virtio_gpu_display_one{}

const VIRTIO_GPU_MAX_SCANOUTS: usize = 16;
/* VIRTIO_GPU_RESP_OK_DISPLAY_INFO */
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_resp_display_info {
    pub hdr:     virtio_gpu_ctrl_hdr,
    pub pmodes:  [virtio_gpu_display_one; VIRTIO_GPU_MAX_SCANOUTS],
}

unsafe impl ByteValued for virtio_gpu_resp_display_info{}

/* data passed in the control vq, 3d related */
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_box {
    pub x: Le32,
    pub y: Le32,
    pub z: Le32,
    pub w: Le32,
    pub h: Le32,
    pub d: Le32,
}

unsafe impl ByteValued for virtio_gpu_box{}

/* VIRTIO_GPU_CMD_TRANSFER_TO_HOST_3D, VIRTIO_GPU_CMD_TRANSFER_FROM_HOST_3D */
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_transfer_host_3d {
    pub hdr:          virtio_gpu_ctrl_hdr,
    pub box_:         virtio_gpu_box,
    pub offset:       Le64,
    pub resource_id:  Le32,
    pub level:        Le32,
    pub stride:       Le32,
    pub layer_stride: Le32,
}

unsafe impl ByteValued for virtio_gpu_transfer_host_3d{}

/* VIRTIO_GPU_CMD_RESOURCE_CREATE_3D */
pub const VIRTIO_GPU_RESOURCE_FLAG_Y_0_TOP : u32 = 1 << 0;
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_resource_create_3d {
    pub hdr:         virtio_gpu_ctrl_hdr,
    pub resource_id: Le32,
    pub target:      Le32,
    pub format:      Le32,
    pub bind:        Le32,
    pub width:       Le32,
    pub height:      Le32,
    pub depth:       Le32,
    pub array_size:  Le32,
    pub last_level:  Le32,
    pub nr_samples:  Le32,
    pub flags:       Le32,
    pub padding:     Le32,
}

unsafe impl ByteValued for virtio_gpu_resource_create_3d{}

/* VIRTIO_GPU_CMD_CTX_CREATE */
#[derive(Copy)]
#[repr(C)]
pub struct virtio_gpu_ctx_create {
    pub hdr:        virtio_gpu_ctrl_hdr,
    pub nlen:       Le32,
    pub padding:    Le32,
    pub debug_name: [u8; 64],
}

unsafe impl ByteValued for virtio_gpu_ctx_create{}

impl Default for virtio_gpu_ctx_create {
    fn default() -> Self {
        // it's safe to initial the C type struct with byte 0
        unsafe { ::std::mem::zeroed() }
    }
}

impl Clone for virtio_gpu_ctx_create {
    fn clone(&self) -> virtio_gpu_ctx_create {
        // clone self
        *self
    }
}

impl fmt::Debug for virtio_gpu_ctx_create {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        // name should not be longer than 64
        let name_size = min(self.nlen.to_native() as usize, 64);
        let debug_name = from_utf8(&self.debug_name[..name_size])
            .unwrap_or("<unkown>");
        f.debug_struct("virtio_gpu_ctx_create")
            .field("hdr", &self.hdr)
            .field("nlen", &self.nlen)
            .field("debug_name", &debug_name)
            .finish()
    }
}

/* VIRTIO_GPU_CMD_CTX_DESTROY */
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_ctx_destroy {
    pub hdr: virtio_gpu_ctrl_hdr,
}

unsafe impl ByteValued for virtio_gpu_ctx_destroy{}

/* VIRTIO_GPU_CMD_CTX_ATTACH_RESOURCE, VIRTIO_GPU_CMD_CTX_DETACH_RESOURCE */
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_ctx_resource {
    pub hdr:            virtio_gpu_ctrl_hdr,
    pub resource_id:    Le32,
    pub padding:        Le32,
}

unsafe impl ByteValued for virtio_gpu_ctx_resource{}

/* VIRTIO_GPU_CMD_SUBMIT_3D */
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_cmd_submit {
    pub hdr:     virtio_gpu_ctrl_hdr,
    pub size:    Le32,
    pub padding: Le32,
}

unsafe impl ByteValued for virtio_gpu_cmd_submit{}

pub const VIRTIO_GPU_CAPSET_VIRGL: u32 =  1;
pub const VIRTIO_GPU_CAPSET_VIRGL2: u32 = 2;

/* VIRTIO_GPU_CMD_GET_CAPSET_INFO */
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_get_capset_info {
    pub hdr:            virtio_gpu_ctrl_hdr,
    pub capset_index:   Le32,
    pub padding:        Le32,
}

unsafe impl ByteValued for virtio_gpu_get_capset_info{}

/* VIRTIO_GPU_RESP_OK_CAPSET_INFO */
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_resp_capset_info {
    pub hdr:                virtio_gpu_ctrl_hdr,
    pub capset_id:          Le32,
    pub capset_max_version: Le32,
    pub capset_max_size:    Le32,
    pub padding:            Le32,
}

unsafe impl ByteValued for virtio_gpu_resp_capset_info{}

/* VIRTIO_GPU_CMD_GET_CAPSET */
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_get_capset {
    pub hdr:                virtio_gpu_ctrl_hdr,
    pub capset_id:          Le32,
    pub capset_version:     Le32,
}

unsafe impl ByteValued for virtio_gpu_get_capset{}

/* VIRTIO_GPU_RESP_OK_CAPSET */
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_resp_capset {
    pub hdr:            virtio_gpu_ctrl_hdr,
    pub capset_data:    PhantomData<[u8]>,
}

unsafe impl ByteValued for virtio_gpu_resp_capset{}

/* VIRTIO_GPU_CMD_GET_EDID */
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_cmd_get_edid {
    pub hdr:     virtio_gpu_ctrl_hdr,
    pub scanout: Le32,
    pub padding: Le32,
}

unsafe impl ByteValued for virtio_gpu_cmd_get_edid{}

/* VIRTIO_GPU_RESP_OK_EDID */
#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct virtio_gpu_resp_edid {
    pub hdr:         virtio_gpu_ctrl_hdr,
    pub size:        Le32,
    pub padding:     Le32,
    pub edid:        [u8; 1024],
}

impl Default for virtio_gpu_resp_edid {
    fn default() -> Self {
        unsafe { ::std::mem::zeroed() }
    }
}

unsafe impl ByteValued for virtio_gpu_resp_edid{}

pub const VIRTIO_GPU_EVENT_DISPLAY: u32 = 1 << 0;
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_config {
    pub events_read:    Le32,
    pub events_clear:   Le32,
    pub num_scanouts:   Le32,
    pub num_capsets:    Le32,
}

unsafe impl ByteValued for virtio_gpu_config{}

/* simple formats for fbcon/X use */
pub const VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM: u32  = 1;
pub const VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM: u32  = 2;
pub const VIRTIO_GPU_FORMAT_A8R8G8B8_UNORM: u32  = 3;
pub const VIRTIO_GPU_FORMAT_X8R8G8B8_UNORM: u32  = 4;
pub const VIRTIO_GPU_FORMAT_R8G8B8A8_UNORM: u32  = 67;
pub const VIRTIO_GPU_FORMAT_X8B8G8R8_UNORM: u32  = 68;
pub const VIRTIO_GPU_FORMAT_A8B8G8R8_UNORM: u32  = 121;
pub const VIRTIO_GPU_FORMAT_R8G8B8X8_UNORM: u32  = 134;


/* VIRTIO_GPU_CMD_RESOURCE_ASSIGN_UUID */
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_resource_assign_uuid {
    pub hdr:         virtio_gpu_ctrl_hdr,
    pub resource_id: Le32,
    pub padding:     Le32,
}

unsafe impl ByteValued for virtio_gpu_resource_assign_uuid{}

/* VIRTIO_GPU_RESP_OK_RESOURCE_UUID */
#[derive(Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct virtio_gpu_resp_resource_uuid {
    pub hdr:    virtio_gpu_ctrl_hdr,
    pub uuid:   [u8; 16],
}

unsafe impl ByteValued for virtio_gpu_resp_resource_uuid{}

#[derive(Debug)]
pub enum VirtioGpuCommandDecodeError {
    InvalidCommand(u32),
    ParserError(GuestMemoryError),
}

impl From<GuestMemoryError> for VirtioGpuCommandDecodeError {
    fn from(e: Error) -> Self {
        ParserError(e)
    }
}

/// VirtioGpuCommand enum
#[derive(Debug, Clone, Copy)]
pub enum VirtioGpuCommand {
    // 2D command
    CmdGetDisplayInfo(virtio_gpu_ctrl_hdr),
    CmdResourceCreate2D(virtio_gpu_resource_create_2d),
    CmdResourceUnref(virtio_gpu_resource_unref),
    CmdSetScanout(virtio_gpu_set_scanout),
    CmdResourceFlush(virtio_gpu_resource_flush),
    CmdTransferToHost2D(virtio_gpu_transfer_to_host_2d),
    CmdResourceAttachBacking(virtio_gpu_resource_attach_backing),
    CmdResourceDetachBacking(virtio_gpu_resource_detach_backing),
    CmdGetCapsetInfo(virtio_gpu_get_capset_info),
    CmdGetCapset(virtio_gpu_get_capset),
    CmdGetEdid(virtio_gpu_cmd_get_edid),


    // 3D command
    CmdCtxCreate(virtio_gpu_ctx_create),
    CmdCtxDestroy(virtio_gpu_ctx_destroy),
    CmdCtxAttachResource(virtio_gpu_ctx_resource),
    CmdCtxDetachResource(virtio_gpu_ctx_resource),
    CmdResourceCreate3D(virtio_gpu_resource_create_3d),
    CmdTransferToHost3D(virtio_gpu_transfer_host_3d),
    CmdTransferFromHost3D(virtio_gpu_transfer_host_3d),
    CmdSubmit3D(virtio_gpu_cmd_submit),


    // Cursor command
    CmdUpdateCursor(virtio_gpu_update_cursor),
    CmdMoveCursor(virtio_gpu_update_cursor),
}

pub type VirtioGpuCommandResult = std::result::Result<VirtioGpuCommand, VirtioGpuCommandDecodeError>;


impl VirtioGpuCommand {
    pub fn size(&self) -> usize {
        match self {
            VirtioGpuCommand::CmdGetDisplayInfo(_)        => size_of::<virtio_gpu_display_one>(),
            VirtioGpuCommand::CmdResourceCreate2D(_)      => size_of::<virtio_gpu_resource_create_2d>(),
            VirtioGpuCommand::CmdResourceUnref(_)         => size_of::<virtio_gpu_resource_unref>(),
            VirtioGpuCommand::CmdSetScanout(_)            => size_of::<virtio_gpu_set_scanout>(),
            VirtioGpuCommand::CmdResourceFlush(_)         => size_of::<virtio_gpu_resource_flush>(),
            VirtioGpuCommand::CmdTransferToHost2D(_)      => size_of::<virtio_gpu_transfer_to_host_2d>(),
            VirtioGpuCommand::CmdResourceAttachBacking(_) => size_of::<virtio_gpu_resource_attach_backing>(),
            VirtioGpuCommand::CmdResourceDetachBacking(_) => size_of::<virtio_gpu_resource_detach_backing>(),
            VirtioGpuCommand::CmdGetCapsetInfo(_)         => size_of::<virtio_gpu_get_capset_info>(),
            VirtioGpuCommand::CmdGetCapset(_)             => size_of::<virtio_gpu_get_capset>(),
            VirtioGpuCommand::CmdGetEdid(_)               => size_of::<virtio_gpu_cmd_get_edid>(),
            VirtioGpuCommand::CmdCtxCreate(_)             => size_of::<virtio_gpu_ctx_create>(),
            VirtioGpuCommand::CmdCtxDestroy(_)            => size_of::<virtio_gpu_ctx_destroy>(),
            VirtioGpuCommand::CmdCtxAttachResource(_)     => size_of::<virtio_gpu_ctx_resource>(),
            VirtioGpuCommand::CmdCtxDetachResource(_)     => size_of::<virtio_gpu_ctx_resource>(),
            VirtioGpuCommand::CmdResourceCreate3D(_)      => size_of::<virtio_gpu_resource_create_3d>(),
            VirtioGpuCommand::CmdTransferToHost3D(_)      => size_of::<virtio_gpu_transfer_host_3d>(),
            VirtioGpuCommand::CmdTransferFromHost3D(_)    => size_of::<virtio_gpu_transfer_host_3d>(),
            VirtioGpuCommand::CmdSubmit3D(_)              => size_of::<virtio_gpu_cmd_submit>(),
            VirtioGpuCommand::CmdUpdateCursor(_)          => size_of::<virtio_gpu_update_cursor>(),
            VirtioGpuCommand::CmdMoveCursor(_)            => size_of::<virtio_gpu_update_cursor>(),
        }
    }

    pub fn decode(
        cmd: &GuestMemoryMmap,
        addr: GuestAddress
    ) -> VirtioGpuCommandResult  {
        use VirtioGpuCommand::*;
        let hdr = cmd.read_obj::<virtio_gpu_ctrl_hdr>(addr)?;
        Ok(match hdr.type_.into() {
            VIRTIO_GPU_CMD_GET_DISPLAY_INFO         => CmdGetDisplayInfo(cmd.read_obj(addr)?),
            VIRTIO_GPU_CMD_RESOURCE_CREATE_2D       => CmdResourceCreate2D(cmd.read_obj(addr)?),
            VIRTIO_GPU_CMD_RESOURCE_UNREF           => CmdResourceUnref(cmd.read_obj(addr)?),
            VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D      => CmdTransferToHost2D(cmd.read_obj(addr)?),
            VIRTIO_GPU_CMD_SET_SCANOUT              => CmdSetScanout(cmd.read_obj(addr)?),
            VIRTIO_GPU_CMD_RESOURCE_FLUSH           => CmdResourceFlush(cmd.read_obj(addr)?),
            VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING  => CmdResourceAttachBacking(cmd.read_obj(addr)?),
            VIRTIO_GPU_CMD_RESOURCE_DETACH_BACKING  => CmdResourceDetachBacking(cmd.read_obj(addr)?),
            VIRTIO_GPU_CMD_GET_CAPSET_INFO          => CmdGetCapsetInfo(cmd.read_obj(addr)?),
            VIRTIO_GPU_CMD_GET_CAPSET               => CmdGetCapset(cmd.read_obj(addr)?),
            VIRTIO_GPU_CMD_GET_EDID                 => CmdGetEdid(cmd.read_obj(addr)?),

            VIRTIO_GPU_CMD_CTX_CREATE               => CmdCtxCreate(cmd.read_obj(addr)?),
            VIRTIO_GPU_CMD_CTX_DESTROY              => CmdCtxDestroy(cmd.read_obj(addr)?),
            VIRTIO_GPU_CMD_CTX_ATTACH_RESOURCE      => CmdCtxAttachResource(cmd.read_obj(addr)?),
            VIRTIO_GPU_CMD_CTX_DETACH_RESOURCE      => CmdCtxDetachResource(cmd.read_obj(addr)?),
            VIRTIO_GPU_CMD_RESOURCE_CREATE_3D       => CmdResourceCreate3D(cmd.read_obj(addr)?),
            VIRTIO_GPU_CMD_TRANSFER_TO_HOST_3D      => CmdTransferToHost3D(cmd.read_obj(addr)?),
            VIRTIO_GPU_CMD_TRANSFER_FROM_HOST_3D    => CmdTransferFromHost3D(cmd.read_obj(addr)?),
            VIRTIO_GPU_CMD_SUBMIT_3D                => CmdSubmit3D(cmd.read_obj(addr)?),

            VIRTIO_GPU_CMD_UPDATE_CURSOR            => CmdUpdateCursor(cmd.read_obj(addr)?),
            VIRTIO_GPU_CMD_MOVE_CURSOR              => CmdMoveCursor(cmd.read_obj(addr)?),

            type_ => return Err(VirtioGpuCommandDecodeError::InvalidCommand(type_)),
        })
    }
}

pub type VirtioGpuResponseResult = ::std::result::Result<VirtioGpuResponse, VirtioGpuResponse>;

impl From<RutabagaError> for VirtioGpuResponse {
    fn from(e: RutabagaError) -> Self {
        VirtioGpuResponse::RutabagaError(e)
    }
}

impl From<GuestMemoryError> for VirtioGpuResponse {
    fn from(e: Error) -> Self {
        VirtioGpuResponse::EncodeError(e)
    }
}

impl From<TryFromIntError> for VirtioGpuResponse {
    fn from(e: TryFromIntError) -> Self {
        VirtioGpuResponse::UnsupportPlatform(e)
    }
}

// Response for the virtio
#[derive(Debug)]
pub enum VirtioGpuResponse {
    OkNoData,
    OkDisplayInfo(Vec<(u32, u32)>),
    OkCapsetInfo {
        capset_id: u32,
        version:   u32,
        size:      u32,
    },
    OkCapset(Vec<u8>),
    OkResourceUuid {
        uuid:   [u8; 16],
    },

    // Err response
    ErrUnspec,
    ErrOutOfMemory,
    ErrInvalidScanoutId,
    ErrInvalidResourceId,
    ErrInvalidContextId,
    ErrInvalidParameter,

    // lib specified error
    TooManyScanout(usize),
    EncodeError(GuestMemoryError),
    RutabagaError(RutabagaError),
    UnsupportPlatform(TryFromIntError),
    InvalidSglistRegion()
}

impl VirtioGpuResponse {
    /// Encode the `VirtioGpuResponse` To virtual queue command
    pub fn encode(
        &self,
        flags:    u32,
        fence_id: u64,
        ctx_id:   u32,
    ) -> Result<Vec<u8>, VirtioGpuResponse> {
        let hdr = virtio_gpu_ctrl_hdr {
            type_:    Le32::from(self.get_resp_command_const()),
            flags:    Le32::from(flags),
            fence_id: Le64::from(fence_id),
            ctx_id:   Le32::from(ctx_id),
            padding:  Default::default(),
        };

        let result: Vec<u8> = match *self {
            VirtioGpuResponse::OkDisplayInfo(ref inner) => {
                if inner.len() > VIRTIO_GPU_MAX_SCANOUTS {
                    return Err(VirtioGpuResponse::TooManyScanout(inner.len()));
                }
                let mut resp = virtio_gpu_resp_display_info {
                    hdr,
                    pmodes: Default::default(),
                };
                for (pmode, &(width, height)) in resp.pmodes.iter_mut().zip(inner) {
                    pmode.r.width = Le32::from(width);
                    pmode.r.height = Le32::from(height);
                    // enable the display screen
                    pmode.enabled = Le32::from(1)
                }

                resp.as_slice().iter().cloned().collect()
            }
            VirtioGpuResponse::OkCapsetInfo{
                capset_id,
                version,
                size
            } => {
                let resp = virtio_gpu_resp_capset_info {
                    hdr,
                    capset_id:          Le32::from(capset_id),
                    capset_max_version: Le32::from(version),
                    capset_max_size:    Le32::from(size),
                    padding: Default::default()
                };
                resp.as_slice().iter().cloned().collect()
            }
            VirtioGpuResponse::OkCapset(ref inner) => {
                let resp = [hdr.as_slice(), inner.as_slice()].concat();
                resp.iter().cloned().collect()
            }
            VirtioGpuResponse::OkResourceUuid{ uuid } => {
                let uuid_resp = virtio_gpu_resp_resource_uuid {
                    hdr,
                    uuid,
                };
                uuid_resp.as_slice().iter().cloned().collect()
            }
            _ => {
                hdr.as_slice().iter().cloned().collect()
            }
        };

        Ok(result)
    }

    pub fn get_resp_command_const(&self) -> u32 {
        match self {
            Self::OkNoData             => VIRTIO_GPU_RESP_OK_NODATA,
            Self::OkDisplayInfo(_)     => VIRTIO_GPU_RESP_OK_DISPLAY_INFO,
            Self::OkCapsetInfo{..}     => VIRTIO_GPU_RESP_OK_CAPSET_INFO,
            Self::OkCapset(_)          => VIRTIO_GPU_RESP_OK_CAPSET,
            Self::OkResourceUuid{..}   => VIRTIO_GPU_RESP_OK_RESOURCE_UUID,

            Self::ErrUnspec            => VIRTIO_GPU_RESP_ERR_UNSPEC,
            Self::ErrOutOfMemory       => VIRTIO_GPU_RESP_ERR_OUT_OF_MEMORY,
            Self::ErrInvalidScanoutId  => VIRTIO_GPU_RESP_ERR_INVALID_SCANOUT_ID,
            Self::ErrInvalidResourceId => VIRTIO_GPU_RESP_ERR_INVALID_RESOURCE_ID,
            Self::ErrInvalidContextId  => VIRTIO_GPU_RESP_ERR_INVALID_CONTEXT_ID,
            Self::ErrInvalidParameter  => VIRTIO_GPU_RESP_ERR_INVALID_PARAMETER,
            _                          => VIRTIO_GPU_RESP_ERR_UNSPEC,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::VirtioGpuResponse;
    use crate::protocol::VIRTIO_GPU_MAX_SCANOUTS;

    #[test]
    fn test_encode_resp() {
        let mut hdr_bytes:Vec<u8> = vec![
            0x00, 0x11, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ];

        let cases : Vec<(VirtioGpuResponse, u8, u8, Vec<u8>)>= vec![
            (VirtioGpuResponse::OkNoData, 0x00, 0x11, vec![]),
            (VirtioGpuResponse::OkDisplayInfo(vec![(1920, 1080)]), 0x01, 0x11,
                [vec![
                    0x00, 0x00, 0x00, 0x00, // x
                    0x00, 0x00, 0x00, 0x00, // y
                    0x80, 0x07, 0x00, 0x00, // width
                    0x38, 0x04, 0x00, 0x00, // height
                    0x01, 0x00, 0x00, 0x00, // enabled
                    0x00, 0x00, 0x00, 0x00, // flags
                ], vec![0; 24 * (VIRTIO_GPU_MAX_SCANOUTS - 1)]].concat()),
            (VirtioGpuResponse::OkCapsetInfo {
                    capset_id: 1,
                    version: 2,
                    size: 3
                }, 0x02, 0x11, vec![
                    0x01, 0x00, 0x00, 0x00,
                    0x02, 0x00, 0x00, 0x00,
                    0x03, 0x00, 0x00, 0x00,
                    0x00, 0x00, 0x00, 0x00,
                ]),
            (VirtioGpuResponse::OkCapset(vec![0x00, 0x01, 0x02]), 0x03, 0x11, vec![
                    0x00, 0x01, 0x02
                ]),
            (VirtioGpuResponse::OkResourceUuid { uuid: [0x02; 16] }, 0x05, 0x11, vec![0x02;16]),
            (VirtioGpuResponse::ErrUnspec, 0x00, 0x12, vec![]),
            (VirtioGpuResponse::ErrOutOfMemory, 0x01, 0x12, vec![]),
            (VirtioGpuResponse::ErrInvalidScanoutId, 0x02, 0x12, vec![]),
            (VirtioGpuResponse::ErrInvalidResourceId, 0x03, 0x12, vec![]),
            (VirtioGpuResponse::ErrInvalidContextId, 0x04, 0x12, vec![]),
            (VirtioGpuResponse::ErrInvalidParameter, 0x05, 0x12, vec![]),
        ];

        for case in cases {
            let resp = case.0;
            hdr_bytes[0] = case.1;
            hdr_bytes[1] = case.2;
            let data: Vec<u8> = hdr_bytes.iter().chain(case.3.iter()).cloned().collect();
            let len = data.len();
            assert_eq!(resp.encode(0, 0, 0).unwrap_or(vec![]), data)
        }

    }
}