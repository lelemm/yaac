//! Read an image on the existing target link and send only a thumbnail to the window.

use std::io::Cursor;
use std::time::{Duration, Instant};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{DynamicImage, ImageEncoder as _, ImageReader, imageops::FilterType};
use serde_json::Value;
use tokio::sync::OwnedSemaphorePermit;
use yaac_protocol::attach::{
    HubUploadPreviewParams, HubUploadPreviewResult, UPLOAD_PREVIEW_MAX_BYTES,
    UPLOAD_PREVIEW_MAX_EDGE,
};
use yaac_protocol::methods::UploadRead;
use yaac_protocol::rpc::{ErrorCode, ErrorObject, RpcId};
use yaac_protocol::{TargetId, UploadId, UploadReadParams, UploadReadResult};

use super::{ClientKey, Core, CoreEvent, Pending, err, to_value};

const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
const CHUNK_BYTES: u64 = 512 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(20);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(110);
pub(super) const CONCURRENT_PREVIEWS: usize = 4;

#[derive(Debug)]
pub(super) struct PreviewRead {
    client: ClientKey,
    id: RpcId,
    upload_id: UploadId,
    edge: u32,
    size: Option<u64>,
    bytes: Vec<u8>,
    deadline: Instant,
    expires: Instant,
    permit: OwnedSemaphorePermit,
}

impl Core {
    pub(super) fn upload_preview(
        &mut self,
        client: ClientKey,
        id: &RpcId,
        params: HubUploadPreviewParams,
    ) -> Result<(), ErrorObject> {
        if params.max_edge == 0 {
            return Err(err(ErrorCode::InvalidParams, "maxEdge must be positive"));
        }
        if !self.targets.contains_key(&params.target) {
            return Err(err(
                ErrorCode::TargetUnknown,
                format!("no target {}", params.target),
            ));
        }
        let permit = self.upload_preview_slots.clone()
            .try_acquire_owned()
            .map_err(|_| {
                err(
                    ErrorCode::LimitExceeded,
                    "too many image previews in progress",
                )
            })?;
        let now = Instant::now();
        self.request_preview_chunk(
            &params.target,
            PreviewRead {
                client,
                id: id.clone(),
                upload_id: params.upload_id,
                edge: params.max_edge.min(UPLOAD_PREVIEW_MAX_EDGE),
                size: None,
                bytes: Vec::new(),
                deadline: now,
                expires: now + TOTAL_TIMEOUT,
                permit,
            },
        )
    }

    fn request_preview_chunk(
        &mut self,
        target: &TargetId,
        mut read: PreviewRead,
    ) -> Result<(), ErrorObject> {
        let params = UploadReadParams {
            upload_id: read.upload_id.clone(),
            offset: read.bytes.len() as u64,
            max_bytes: CHUNK_BYTES,
        };
        read.deadline = (Instant::now() + READ_TIMEOUT).min(read.expires);
        if self.send::<UploadRead>(target, &params, Pending::UploadPreview(read)) {
            self.flush(target);
            Ok(())
        } else {
            Err(err(ErrorCode::TargetOffline, "the target is not connected"))
        }
    }

    pub(super) fn on_preview_chunk(
        &mut self,
        target: &TargetId,
        mut read: PreviewRead,
        outcome: Result<Value, ErrorObject>,
    ) {
        if !self.clients.contains_key(&read.client) {
            return;
        }
        // Reserve method_not_found for an older hub. It must not make the window fall back
        // when the upstream target itself lacks upload/read.
        let outcome = outcome.map_err(|e| {
            if e.code == ErrorCode::MethodNotFound {
                err(
                    ErrorCode::CapabilityMissing,
                    "the target cannot read stored uploads",
                )
            } else {
                e
            }
        });
        let next = outcome
            .and_then(|wire| self.decode_typed::<UploadRead>(target, wire))
            .and_then(|chunk| read.append(chunk));
        let client = read.client;
        let id = read.id.clone();
        match next {
            Ok(true) => {
                if let Err(error) = self.request_preview_chunk(target, read) {
                    self.reply(client, id, Err(error));
                }
            }
            Ok(false) => {
                let PreviewRead {
                    bytes,
                    edge,
                    permit,
                    ..
                } = read;
                self.pool.submit(move || {
                    let outcome = make_preview(bytes, edge).map(|png| {
                        to_value(&HubUploadPreviewResult {
                            png_base64: STANDARD.encode(png),
                        })
                    });
                    drop(permit);
                    Some(CoreEvent::UploadPreviewReady {
                        client,
                        id,
                        outcome,
                    })
                });
            }
            Err(error) => self.reply(client, id, Err(error)),
        }
    }

    pub(super) fn close_preview_read(&self, read: PreviewRead) {
        self.reply(
            read.client,
            read.id,
            Err(err(
                ErrorCode::TargetOffline,
                "the connection to the target dropped",
            )),
        );
    }

    pub(super) fn cancel_preview_reads(&mut self, client: ClientKey) {
        for conn in self
            .targets
            .values_mut()
            .filter_map(|target| target.conn.as_mut())
        {
            conn.pending.retain(|_, pending| !matches!(pending, Pending::UploadPreview(read) if read.client == client));
        }
    }

    pub(super) fn expire_preview_reads(&mut self, now: Instant) {
        let mut expired = Vec::new();
        for conn in self
            .targets
            .values_mut()
            .filter_map(|target| target.conn.as_mut())
        {
            conn.pending.retain(|_, pending| {
                if let Pending::UploadPreview(read) = pending
                    && now >= read.deadline
                {
                    expired.push((read.client, read.id.clone()));
                    return false;
                }
                true
            });
        }
        for (client, id) in expired {
            self.reply(
                client,
                id,
                Err(err(
                    ErrorCode::InternalError,
                    "timed out reading the image from the target",
                )),
            );
        }
    }
}

impl PreviewRead {
    fn append(&mut self, reply: UploadReadResult) -> Result<bool, ErrorObject> {
        if !reply.present {
            return Err(err(
                ErrorCode::UploadNotFound,
                "The file is no longer available.",
            ));
        }
        if reply.size > MAX_SOURCE_BYTES {
            return Err(err(
                ErrorCode::LimitExceeded,
                "This file exceeds the 64 MiB preview limit.",
            ));
        }
        let incomplete = || {
            err(
                ErrorCode::InternalError,
                "The target returned an incomplete image.",
            )
        };
        if self.size.is_some_and(|size| size != reply.size)
            || reply.data_base64.len() as u64 > CHUNK_BYTES.div_ceil(3) * 4
        {
            return Err(incomplete());
        }
        let chunk = STANDARD.decode(reply.data_base64).map_err(|_| {
            err(
                ErrorCode::InternalError,
                "The target returned invalid image data.",
            )
        })?;
        let total = (self.bytes.len() + chunk.len()) as u64;
        if chunk.is_empty()
            || chunk.len() as u64 > CHUNK_BYTES
            || total > reply.size
            || reply.more != (total < reply.size)
        {
            return Err(incomplete());
        }
        self.size = Some(reply.size);
        self.bytes.extend_from_slice(&chunk);
        Ok(reply.more)
    }
}

fn make_preview(bytes: Vec<u8>, edge: u32) -> Result<Vec<u8>, ErrorObject> {
    let decode_error = |e: image::ImageError| {
        err(
            ErrorCode::InvalidParams,
            format!("Could not decode the image. {e}"),
        )
    };
    let mut reader = ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|e| decode_error(e.into()))?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(160 * 1024 * 1024);
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    reader.limits(limits);
    let image = reader.decode().map_err(decode_error)?;
    drop(bytes);
    let image = resize(image, edge);
    let png = encode_png(&image)?;
    if png.len() <= UPLOAD_PREVIEW_MAX_BYTES {
        return Ok(png);
    }
    // 128×128 RGBA pixels fit the byte budget even when they barely compress.
    let png = encode_png(&resize(image, 128))?;
    if png.len() > UPLOAD_PREVIEW_MAX_BYTES {
        return Err(err(
            ErrorCode::LimitExceeded,
            "The image preview exceeds the size limit.",
        ));
    }
    Ok(png)
}

fn resize(image: DynamicImage, edge: u32) -> DynamicImage {
    if image.width() > edge || image.height() > edge {
        image.resize(edge, edge, FilterType::Triangle)
    } else {
        image
    }
}

fn encode_png(image: &DynamicImage) -> Result<Vec<u8>, ErrorObject> {
    let rgba = image.to_rgba8();
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| {
            err(
                ErrorCode::InternalError,
                format!("Could not encode the preview. {e}"),
            )
        })?;
    Ok(png)
}
