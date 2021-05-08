use std::num::NonZeroU32;
use rutabaga_gfx::{Rutabaga, ResourceCreate3D, RUTABAGA_PIPE_TEXTURE_2D, RUTABAGA_PIPE_BIND_RENDER_TARGET, RutabagaIovec, Transfer3D, RutabagaBuilder, RutabagaFenceData, VirglRendererFlags, RutabagaComponentType, RutabagaError};
use std::collections::BTreeMap;
use vm_memory::{GuestMemoryMmap, GuestAddress, GuestMemory, VolatileSlice};
use std::os::raw::c_void;
use crate::protocol::*;
use crate::protocol::VirtioGpuResponse::{OkNoData, OkCapsetInfo, OkCapset, ErrInvalidResourceId, OkDisplayInfo, OkResourceUuid, OkEdid, ErrUnspec};
use std::fs::read_to_string;
use std::cell::RefCell;
use std::rc::Rc;
use gpu_display::GpuDisplay;

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum GpuMode {
    Mode2D,
    Mode3D,
}

#[derive(Copy, Clone, Debug)]
pub struct GpuParameter {
    pub display_width:            u32,
    pub display_height:           u32,
    pub renderer_use_egl:         bool,
    pub renderer_use_gles:        bool,
    pub renderer_use_glx:         bool,
    pub renderer_use_surfaceless: bool,
    pub mode:                     GpuMode,
}

const DEFAULT_DSIPLAY_WIDTH: u32  = 1920;
const DEFAULT_DISPLAY_HEIGHT: u32 = 1080;

/// Warn: it's unsafe to used in thread, only be used with Mutex
unsafe impl Send for VirtioGpu {}

impl Default for GpuParameter {
    fn default() -> Self {
        Self {
            display_width: DEFAULT_DSIPLAY_WIDTH,
            display_height: DEFAULT_DISPLAY_HEIGHT,
            renderer_use_egl: true,
            renderer_use_gles: true,
            renderer_use_glx: true,
            renderer_use_surfaceless: true,
            mode: GpuMode::Mode3D
        }
    }
}

pub struct VirtioGpuResource {
    resource_id: u32,
    width: u32,
    height: u32,
    size: u64,
}

impl VirtioGpuResource {
    /// Creates a new VirtioGpuResource with the given metadata.  Width and height are used by the
    /// display, while size is useful for hypervisor mapping.
    pub fn new(resource_id: u32, width: u32, height: u32, size: u64) -> VirtioGpuResource {
        VirtioGpuResource {
            resource_id,
            width,
            height,
            size,
        }
    }

    /// Returns the dimensions of the VirtioGpuResource.
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

pub struct VirtioGpu {
    pub display:         Rc<RefCell<GpuDisplay>>,
    display_width:       u32,
    display_height:      u32,
    scanout_resource_id: Option<NonZeroU32>,
    scanout_surface_id:  Option<u32>,
    cursor_resource_id:  Option<NonZeroU32>,
    cursor_surface_id:   Option<u32>,
    rutabaga:            Rutabaga,
    resources:           BTreeMap<u32, VirtioGpuResource>,
}

fn sglist_to_rutabaga_iovecs(vecs: &[(GuestAddress, usize)], mem: &GuestMemoryMmap) -> Result<Vec<RutabagaIovec>, VirtioGpuResponse> {
    // validate sglist range
    if vecs
        .iter()
        .any(|&(addr, len)| mem.get_slice(addr, len).is_err()) {
        return Err(VirtioGpuResponse::InvalidSglistRegion());
    }

    let mut iovecs: Vec<RutabagaIovec> = Vec::new();
    for &(addr, len) in vecs {
        // it is safe to unwrap host address because we have already checked
        let address = mem.get_host_address(addr).unwrap();
        iovecs.push(RutabagaIovec {
            base: address as *mut c_void,
            len,
        })
    }

    Ok(iovecs)
}

fn transfer_host_3d_to_transfer_3d(
    cmd: virtio_gpu_transfer_host_3d
) -> Transfer3D {
    Transfer3D {
        x: cmd.box_.x.to_native(),
        y: cmd.box_.y.to_native(),
        z: cmd.box_.z.to_native(),
        w: cmd.box_.w.to_native(),
        h: cmd.box_.h.to_native(),
        d: cmd.box_.d.to_native(),
        level: cmd.level.to_native(),
        stride: cmd.stride.to_native(),
        layer_stride: cmd.layer_stride.to_native(),
        offset: cmd.offset.to_native(),
    }
}

impl VirtioGpu {
    pub fn new(
        gpu_parameter: GpuParameter,
    ) -> Result<Self, RutabagaError> {
        let display = GpuDisplay::open_x(None).unwrap();
        let virtglrenderer_flags = VirglRendererFlags::new()
            .use_egl(gpu_parameter.renderer_use_egl)
            .use_gles(gpu_parameter.renderer_use_gles)
            .use_glx(gpu_parameter.renderer_use_glx)
            .use_surfaceless(gpu_parameter.renderer_use_surfaceless);

        let component = match gpu_parameter.mode {
            GpuMode::Mode2D => RutabagaComponentType::Rutabaga2D,
            GpuMode::Mode3D => RutabagaComponentType::VirglRenderer,
        };

        let rutabaga_builder = RutabagaBuilder::new(component)
            .set_virglrenderer_flags(virtglrenderer_flags);

        let rutabaga = rutabaga_builder.build()?;

        Ok(Self {
            display: Rc::new(RefCell::new(display)),
            display_width: gpu_parameter.display_width,
            display_height: gpu_parameter.display_height,
            scanout_resource_id: None,
            scanout_surface_id: None,
            cursor_resource_id: None,
            cursor_surface_id: None,
            rutabaga,
            resources: Default::default()
        })
    }

    pub fn display(&mut self) -> &Rc<RefCell<GpuDisplay>> { &self.display }

    /// Gets the list of supported display resolutions as a slice of `(width, height)` tuples.
    pub fn display_info(&self) -> [(u32, u32); 1] {
        [(self.display_width, self.display_height)]
    }

    pub fn process_display(&mut self) -> bool {
        let mut display = self.display.borrow_mut();
        display.dispatch_events();
        self.scanout_surface_id
            .map(|s| display.close_requested(s))
            .unwrap_or(false)
    }

    fn resource_create_3d(&mut self, resource_id: u32, resource_create_3d: ResourceCreate3D) -> VirtioGpuResponseResult {
        self.rutabaga
            .resource_create_3d(resource_id, resource_create_3d)?;

        let resource = VirtioGpuResource::new(
            resource_id,
            resource_create_3d.width,
            resource_create_3d.height,
            0,
        );

        self.resources.insert(resource_id, resource);

        match self.rutabaga.query(resource_id) {
            Ok(_) => Ok(VirtioGpuResponse::ErrInvalidResourceId),
            Err(_) => Ok(OkNoData)
        }
    }

    pub fn cmd_get_display_info(&mut self, cmd: virtio_gpu_ctrl_hdr) -> VirtioGpuResponseResult {
        Ok(OkDisplayInfo(Vec::from([(self.display_width, self.display_height)])))
    }

    pub fn cmd_resource_create_2d(&mut self, cmd: virtio_gpu_resource_create_2d) -> VirtioGpuResponseResult {
        let resource_create_3d = ResourceCreate3D {
            target: RUTABAGA_PIPE_TEXTURE_2D,
            format: cmd.format.to_native(),
            bind: RUTABAGA_PIPE_BIND_RENDER_TARGET,
            width: cmd.width.to_native(),
            height: cmd.height.to_native(),
            depth: 1,
            array_size: 1,
            last_level: 0,
            nr_samples: 0,
            flags: 0,
        };
        self.resource_create_3d(cmd.resource_id.to_native(), resource_create_3d)
    }

    pub fn cmd_resource_create_3d(&mut self, cmd: virtio_gpu_resource_create_3d) -> VirtioGpuResponseResult {
        let resource_create_3d = ResourceCreate3D {
            target: cmd.target.to_native(),
            format: cmd.format.to_native(),
            bind: cmd.bind.to_native(),
            width: cmd.width.to_native(),
            height: cmd.height.to_native(),
            depth: cmd.depth.to_native(),
            array_size: cmd.array_size.to_native(),
            last_level: cmd.last_level.to_native(),
            nr_samples: cmd.nr_samples.to_native(),
            flags: cmd.flags.to_native(),
        };
        self.resource_create_3d(cmd.resource_id.to_native(), resource_create_3d)
    }

    pub fn cmd_resource_unref(&mut self, cmd: virtio_gpu_resource_unref) -> VirtioGpuResponseResult {
        self.rutabaga.unref_resource(cmd.resource_id.to_native())?;
        self.resources
            .remove(&cmd.resource_id.to_native())
            .ok_or(ErrInvalidResourceId)?;
        Ok(OkNoData)
    }

    pub fn cmd_context_create(&mut self, cmd: virtio_gpu_ctx_create) -> VirtioGpuResponseResult {
        self.rutabaga.create_context(cmd.hdr.ctx_id.to_native(), 0)?;
        Ok(OkNoData)
    }

    pub fn cmd_context_destroy(&mut self, cmd: virtio_gpu_ctx_destroy) -> VirtioGpuResponseResult {
        self.rutabaga.destroy_context(cmd.hdr.ctx_id.to_native())?;
        Ok(OkNoData)
    }

    pub fn cmd_get_edid(&mut self, cmd: virtio_gpu_cmd_get_edid) -> VirtioGpuResponseResult {
        let mut edid = [0u8; 1024];
        let edid_vec: Vec<u8> = vec![
            // 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00, 0x09, 0xe5, 0xdf, 0x06, 0x00, 0x00, 0x00, 0x00,
            // 0x01, 0x1a, 0x01, 0x04, 0xa5, 0x1f, 0x11, 0x78, 0x02, 0x86, 0x31, 0xa3, 0x54, 0x4e, 0x9b, 0x25,
            // 0x0e, 0x50, 0x54, 0x00, 0x00, 0x00, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
            // 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x3c, 0x37, 0x80, 0xde, 0x70, 0x38, 0x14, 0x40, 0x3c, 0x20,
            // 0x36, 0x00, 0x35, 0xad, 0x10, 0x00, 0x00, 0x1a, 0x30, 0x2c, 0x80, 0xde, 0x70, 0x38, 0x14, 0x40,
            // 0x30, 0x20, 0x36, 0x00, 0x35, 0xad, 0x10, 0x00, 0x00, 0x1a, 0x00, 0x00, 0x00, 0xfe, 0x00, 0x42,
            // 0x4f, 0x45, 0x20, 0x43, 0x51, 0x0a, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x00, 0x00, 0x00, 0xfe,
            // 0x00, 0x48, 0x56, 0x31, 0x34, 0x30, 0x46, 0x48, 0x4d, 0x2d, 0x4e, 0x36, 0x31, 0x0a, 0x00, 0x49,
            0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00, 0x10, 0xac, 0xc0, 0xa0, 0x4c, 0x55, 0x36, 0x30,
            0x2d, 0x18, 0x01, 0x03, 0x80, 0x35, 0x1e, 0x78, 0xea, 0xe2, 0x45, 0xa8, 0x55, 0x4d, 0xa3, 0x26,
            0x0b, 0x50, 0x54, 0xa5, 0x4b, 0x00, 0x71, 0x4f, 0x81, 0x80, 0xa9, 0xc0, 0xa9, 0x40, 0xd1, 0xc0,
            0xe1, 0x00, 0x01, 0x01, 0x01, 0x01, 0xa3, 0x66, 0x00, 0xa0, 0xf0, 0x70, 0x1f, 0x80, 0x30, 0x20,
            0x35, 0x00, 0x0f, 0x28, 0x21, 0x00, 0x00, 0x1a, 0x00, 0x00, 0x00, 0xff, 0x00, 0x50, 0x32, 0x50,
            0x43, 0x32, 0x34, 0x42, 0x34, 0x30, 0x36, 0x55, 0x4c, 0x0a, 0x00, 0x00, 0x00, 0xfc, 0x00, 0x44,
            0x45, 0x4c, 0x4c, 0x20, 0x50, 0x32, 0x34, 0x31, 0x35, 0x51, 0x0a, 0x20, 0x00, 0x00, 0x00, 0xfd,
            0x00, 0x1d, 0x4c, 0x1e, 0x8c, 0x1e, 0x00, 0x0a, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x01, 0x96,
            0x02, 0x03, 0x2a, 0xf1, 0x53, 0x90, 0x05, 0x04, 0x02, 0x07, 0x16, 0x01, 0x14, 0x1f, 0x12, 0x13,
            0x27, 0x20, 0x21, 0x22, 0x03, 0x06, 0x11, 0x15, 0x23, 0x09, 0x07, 0x07, 0x6d, 0x03, 0x0c, 0x00,
            0x10, 0x00, 0x30, 0x3c, 0x20, 0x00, 0x60, 0x03, 0x02, 0x01, 0x02, 0x3a, 0x80, 0x18, 0x71, 0x38,
            0x2d, 0x40, 0x58, 0x2c, 0x25, 0x00, 0x0f, 0x28, 0x21, 0x00, 0x00, 0x1f, 0x01, 0x1d, 0x80, 0x18,
            0x71, 0x1c, 0x16, 0x20, 0x58, 0x2c, 0x25, 0x00, 0x0f, 0x28, 0x21, 0x00, 0x00, 0x9e, 0x04, 0x74,
            0x00, 0x30, 0xf2, 0x70, 0x5a, 0x80, 0xb0, 0x58, 0x8a, 0x00, 0x0f, 0x28, 0x21, 0x00, 0x00, 0x1e,
            0x56, 0x5e, 0x00, 0xa0, 0xa0, 0xa0, 0x29, 0x50, 0x30, 0x20, 0x35, 0x00, 0x0f, 0x28, 0x21, 0x00,
            0x00, 0x1a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf9,
        ];
        for (pos, e) in edid_vec.iter().enumerate() {
            edid[pos] = *e;
        }
        Ok(OkEdid {
            size: edid_vec.len() as u32,
            edid
        })
    }

    pub fn cmd_get_capset_info(&mut self, cmd: virtio_gpu_get_capset_info) -> VirtioGpuResponseResult {
        let (capset_id, version, size) = self.rutabaga.get_capset_info(cmd.capset_index.to_native())?;
        Ok(OkCapsetInfo {
            capset_id,
            version,
            size
        })
    }

    /// get rubataga capaset
    pub fn cmd_get_capset(&mut self, cmd: virtio_gpu_get_capset) -> VirtioGpuResponseResult {
        let capset = self.rutabaga.get_capset(cmd.capset_id.to_native(), cmd.capset_version.to_native())?;
        Ok(OkCapset(capset))
    }

    /// Attempts to import the given resource into the display, otherwise falls back to rutabaga
    /// copies.
    pub fn flush_resource_to_surface(
        &mut self,
        resource_id: u32,
        surface_id: u32,
    ) -> VirtioGpuResponseResult {
        if let Some(import_id) = self.import_to_display(resource_id) {
            self.display.borrow_mut().flip_to(surface_id, import_id);
            return Ok(OkNoData);
        }

        if !self.resources.contains_key(&resource_id) {
            return Err(ErrInvalidResourceId);
        }

        // Import failed, fall back to a copy.
        let mut display = self.display.borrow_mut();
        // Prevent overwriting a buffer that is currently being used by the compositor.
        if display.next_buffer_in_use(surface_id.clone()) {
            return Ok(OkNoData);
        }

        let fb = display
            .framebuffer_region(surface_id, 0, 0, self.display_width.clone(), self.display_height.clone())
            .ok_or(ErrUnspec)?;

        let mut transfer = Transfer3D::new_2d(0, 0, self.display_width.clone(), self.display_height.clone());
        transfer.stride = fb.stride();
        self.rutabaga
            .transfer_read(0, resource_id, transfer, Some(fb.as_volatile_slice()))?;
        display.flip(surface_id);

        Ok(OkNoData)
    }

    /// flush resource screen
    #[allow(unused_variables)]
    pub fn cmd_flush_resource(&mut self, cmd: virtio_gpu_resource_flush) -> VirtioGpuResponseResult {
        let resource_id = cmd.resource_id.to_native();
        if resource_id == 0 {
            return Ok(OkNoData);
        }

        if let (Some(scanout_resource_id), Some(scanout_surface_id)) =
            (self.scanout_resource_id, self.scanout_surface_id)
        {
            if scanout_resource_id.get() == cmd.resource_id.to_native() {
                self.flush_resource_to_surface(resource_id, scanout_surface_id)?;
            }
        }

        if let (Some(cursor_resource_id), Some(cursor_surface_id)) =
            (self.cursor_resource_id, self.cursor_surface_id)
        {
            if cursor_resource_id.get() == resource_id {
                self.flush_resource_to_surface(resource_id, cursor_surface_id)?;
            }
        }

        Ok(OkNoData)
    }
    pub fn import_to_display(&mut self, resource_id: u32) -> Option<u32> { None }


    /// set the scanout surface
    pub fn cmd_set_scanout(&mut self, cmd: virtio_gpu_set_scanout) -> VirtioGpuResponseResult {
        let resource_id = cmd.resource_id.to_native();

        if resource_id == 0 {
            // TODO: if we implement the display protocol, try to use it
            self.scanout_surface_id = None;
            self.scanout_resource_id = None;
            return Ok(OkNoData);
        }

        #[allow(unused_variables)]
        let resource = self
            .resources
            .get_mut(&cmd.resource_id.to_native())
            .ok_or(ErrInvalidResourceId)?;

        self.scanout_resource_id = NonZeroU32::new(resource_id);
        if self.scanout_surface_id.is_none() {
            self.scanout_surface_id = Some(cmd.scanout_id.to_native());
        }

        Ok(OkNoData)
    }

    pub fn cmd_resource_attach_backing(
        &mut self,
        cmd: virtio_gpu_resource_attach_backing,
        data: Vec<RutabagaIovec>
    ) -> VirtioGpuResponseResult {
        self.rutabaga.attach_backing(cmd.resource_id.to_native(), data)?;

        Ok(OkNoData)
    }

    pub fn cmd_resource_detach_backing(
        &mut self,
        cmd: virtio_gpu_resource_detach_backing
    ) -> VirtioGpuResponseResult {
        self.rutabaga.detach_backing(cmd.resource_id.to_native())?;
        Ok(OkNoData)
    }

    pub fn cmd_ctx_attach_resource(
        &mut self,
        cmd: virtio_gpu_ctx_resource
    ) -> VirtioGpuResponseResult {
        self.rutabaga.context_attach_resource(cmd.hdr.ctx_id.to_native(), cmd.resource_id.to_native())?;
        Ok(OkNoData)
    }

    pub fn cmd_ctx_detach_resource(
        &mut self,
        cmd: virtio_gpu_ctx_resource
    ) -> VirtioGpuResponseResult {
        self.rutabaga.context_detach_resource(cmd.hdr.ctx_id.to_native(), cmd.resource_id.to_native())?;
        Ok(OkNoData)
    }

    pub fn cmd_submit_3d(
        &mut self,
        cmd: virtio_gpu_cmd_submit,
        data: &mut [u8]
    ) -> VirtioGpuResponseResult {
        self.rutabaga.submit_command(cmd.hdr.ctx_id.to_native(), data)?;
        Ok(OkNoData)
    }

    pub fn cmd_transfer_to_host_2d(
        &mut self,
        cmd: virtio_gpu_transfer_to_host_2d
    ) -> VirtioGpuResponseResult {
        let resource_id = cmd.resource_id.to_native();
        let transfer = Transfer3D::new_2d(
            cmd.r.x.to_native(),
            cmd.r.y.to_native(),
            cmd.r.width.to_native(),
            cmd.r.height.to_native()
        );

        self.rutabaga.transfer_write(cmd.hdr.ctx_id.to_native(), resource_id, transfer)?;
        Ok(OkNoData)
    }


    pub fn cmd_transfer_to_host_3d(
        &mut self,
        cmd: virtio_gpu_transfer_host_3d
    ) -> VirtioGpuResponseResult {
        let resource_id = cmd.resource_id.to_native();
        let transfer = transfer_host_3d_to_transfer_3d(cmd);
        self.rutabaga.transfer_write(cmd.hdr.ctx_id.to_native(), resource_id, transfer)?;
        Ok(OkNoData)
    }

    pub fn cmd_resource_assign_uuid(&self, cmd: virtio_gpu_resource_assign_uuid) -> VirtioGpuResponseResult {
        let resource_id = cmd.resource_id.to_native();
        if !self.resources.contains_key(&resource_id) {
            return Err(ErrInvalidResourceId);
        }

        let mut uuid: [u8; 16] = [0; 16];
        for (idx, byte) in resource_id.to_be_bytes().iter().enumerate() {
            uuid[12 + idx] = *byte;
        }
        Ok(OkResourceUuid { uuid })
    }

    #[allow(unused_variablesb)]
    pub fn cmd_transfer_from_host_3d(
        &mut self,
        cmd: virtio_gpu_transfer_host_3d,
        buf: Option<VolatileSlice>
    ) -> VirtioGpuResponseResult {
        let resource_id = cmd.resource_id.to_native();
        let transfer = transfer_host_3d_to_transfer_3d(cmd);
        self.rutabaga.transfer_read(cmd.hdr.ctx_id.to_native(), resource_id, transfer, None)?;
        Ok(OkNoData)
    }

    /// TODO: not implement, just return OkNoData
    pub fn cmd_move_curosr(
        &mut self,
        cmd: virtio_gpu_update_cursor
    ) -> VirtioGpuResponseResult {
        let x = cmd.pos.x.to_native();
        let y = cmd.pos.y.to_native();
        if let Some(cursor_surface_id) = self.cursor_surface_id {
            if let Some(scanout_surface_id) = self.scanout_surface_id {
                let mut display = self.display.borrow_mut();
                display.set_position(cursor_surface_id, x, y);
                display.commit(scanout_surface_id);
            }
        }
        Ok(OkNoData)
    }

    #[allow(unused_variables)]
    pub fn cmd_update_cursor(
        &mut self,
        cmd: virtio_gpu_update_cursor
    ) -> VirtioGpuResponseResult {
        let resource_id = cmd.resource_id.to_native();
        let y = cmd.pos.y.to_native();
        let x = cmd.pos.x.to_native();
        if resource_id == 0 {
            if let Some(surface_id) = self.cursor_surface_id.take() {
                self.display.borrow_mut().release_surface(surface_id);
            }
            self.cursor_resource_id = None;
            return Ok(OkNoData);
        }

        let (resource_width, resource_height) = self
            .resources
            .get_mut(&resource_id)
            .ok_or(ErrInvalidResourceId)?
            .dimensions();

        self.cursor_resource_id = NonZeroU32::new(resource_id);

        if self.cursor_surface_id.is_none() {
            self.cursor_surface_id = Some(self.display.borrow_mut().create_surface(
                self.scanout_surface_id,
                resource_width,
                resource_height,
            ).map_err(VirtioGpuResponse::DisplayErr)?);
        }

        let cursor_surface_id = self.cursor_surface_id.unwrap();
        self.display
            .borrow_mut()
            .set_position(cursor_surface_id, x, y);

        // Gets the resource's pixels into the display by importing the buffer.
        if let Some(import_id) = self.import_to_display(resource_id) {
            self.display
                .borrow_mut()
                .flip_to(cursor_surface_id, import_id);
            return Ok(OkNoData);
        }

        // Importing failed, so try copying the pixels into the surface's slower shared memory
        // framebuffer.
        if let Some(fb) = self.display.borrow_mut().framebuffer(cursor_surface_id) {
            let mut transfer = Transfer3D::new_2d(0, 0, resource_width, resource_height);
            transfer.stride = fb.stride();
            self.rutabaga
                .transfer_read(0, resource_id, transfer, Some(fb.as_volatile_slice()))?;
        }
        self.display.borrow_mut().flip(cursor_surface_id);
        Ok(OkNoData)
    }

    /// poll the fenced data
    #[allow(unused_variables)]
    pub fn fence_poll(&mut self) -> Vec<RutabagaFenceData> {
        self.rutabaga.poll()
    }

    pub fn force_ctx_0(&mut self) {
        self.rutabaga.force_ctx_0()
    }

    /// create fence for ctx
    pub fn create_fence(&mut self, request_fence_data: RutabagaFenceData) -> VirtioGpuResponseResult {
        self.rutabaga.create_fence(request_fence_data)?;
        Ok(OkNoData)
    }
}


#[cfg(test)]
pub(crate) mod tests {
    use crate::virtio_gpu::GpuParameter;
    use crate::VirtioGpu;
    use gpu_display::GpuDisplay;

    #[test]
    fn test_new_virtio_gpu() {
        let gpu_parameter: GpuParameter = Default::default();
        let virtio_gpu = VirtioGpu::new(gpu_parameter).map_err(|e| {
                panic!("Gpu: create new virtio gpu failed, err: {:?}", e);
                e
            }).unwrap();
    }
}


