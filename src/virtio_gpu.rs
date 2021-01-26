use std::num::NonZeroU32;
use rutabaga_gfx::{Rutabaga, ResourceCreate3D, RUTABAGA_PIPE_TEXTURE_2D, RUTABAGA_PIPE_BIND_RENDER_TARGET, RutabagaIovec, Transfer3D, RutabagaBuilder, RutabagaFenceData, VirglRendererFlags, RutabagaComponentType, RutabagaError};
use std::collections::BTreeMap;
use vm_memory::{GuestMemoryMmap, GuestAddress, GuestMemory, VolatileSlice};
use std::os::raw::c_void;
use crate::protocol::*;
use crate::protocol::VirtioGpuResponse::{OkNoData, OkCapsetInfo, OkCapset, ErrInvalidResourceId, OkDisplayInfo, OkResourceUuid};

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

const DEFAULT_DSIPLAY_WIDTH: u32  = 900;
const DEFAULT_DISPLAY_HEIGHT: u32 = 600;

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
    display_width:       u32,
    display_height:      u32,
    scanout_resource_id: Option<NonZeroU32>,
    scanout_surface_id:  Option<u32>,
    cursor_resource_id:  Option<NonZeroU32>,
    cursor_surface_id:   u32,
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
            display_width: gpu_parameter.display_width,
            display_height: gpu_parameter.display_height,
            scanout_resource_id: None,
            scanout_surface_id: None,
            cursor_resource_id: None,
            cursor_surface_id: 0,
            rutabaga,
            resources: Default::default()
        })
    }

    fn resource_create_3d(&mut self, resource_id: u32, resource_create_3d: ResourceCreate3D) -> VirtioGpuResponseResult {
        self.rutabaga
            .resource_create_3d(resource_id, resource_create_3d)?;

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
            bind: cmd.width.to_native(),
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

    /// flush resource screen
    #[allow(unused_variables)]
    pub fn cmd_flush_resource(&mut self, cmd: virtio_gpu_resource_flush) -> VirtioGpuResponseResult {
        Ok(OkNoData)
    }

    /// set the scanout surface
    pub fn cmd_set_scanout(&mut self, cmd: virtio_gpu_set_scanout) -> VirtioGpuResponseResult {
        let resource_id = cmd.resource_id.to_native();

        if resource_id == 0 {
            // TODO: if we implement the display protocol, try to use it
            self.scanout_surface_id = None;
            self.scanout_resource_id = None;
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
        Ok(OkNoData)
    }

    /// TODO: not implement, just return OkNoData
    #[allow(unused_variables)]
    pub fn cmd_update_cursor(
        &mut self,
        cmd: virtio_gpu_update_cursor
    ) -> VirtioGpuResponseResult {
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

    #[test]
    fn test_new_virtio_gpu() {
        let gpu_parameter: GpuParameter = Default::default();
        let virtio_gpu = VirtioGpu::new(gpu_parameter).map_err(|e| {
                panic!("Gpu: create new virtio gpu failed, err: {:?}", e);
                e
            }).unwrap();
    }
}


