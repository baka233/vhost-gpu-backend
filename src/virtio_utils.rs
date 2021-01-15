use crate::protocol::{virtio_gpu_ctrl_hdr, VIRTIO_GPU_FLAG_FENCE};

pub fn is_fence(hdr: virtio_gpu_ctrl_hdr) -> bool {
    hdr.flags.to_native() & VIRTIO_GPU_FLAG_FENCE != 0
}
