//! Image routing (docs/06-attachments.md#images-are-not-ordinary-attachments): an image is sent
//! as image input only when the selected model is known to see images.

use yaac_protocol::{Capability, ModelInfo, Support};

use crate::state::SendAs;

/// How one attachment will be sent, and what its chip says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageRoute {
    /// An `image` part: the model sees it.
    AsImage,
    /// A `file` part, with the warning "this model cannot see images — sent as a file".
    AsFileCannotSee,
    /// Support is unknown and the user has not chosen: the chip asks "send as image or file?".
    /// Sending is held until they choose. YAAC does not guess.
    Ask,
    /// A `file` part (not an image, or the user chose file).
    AsFile,
}

impl ImageRoute {
    /// Whether the message can be sent with this attachment as it is.
    pub fn ready(self) -> bool {
        !matches!(self, ImageRoute::Ask)
    }
}

/// Route one attachment by the effective image support of the model selected **now**.
pub fn route(is_image: bool, support: &Support, choice: Option<SendAs>) -> ImageRoute {
    if !is_image {
        return ImageRoute::AsFile;
    }
    match support {
        Support::Supported => ImageRoute::AsImage,
        Support::Unsupported => ImageRoute::AsFileCannotSee,
        // Unknown, or a value this build does not know: the user decides.
        _ => match choice {
            Some(SendAs::Image) => ImageRoute::AsImage,
            Some(SendAs::File) => ImageRoute::AsFile,
            None => ImageRoute::Ask,
        },
    }
}

/// A model's image support as its descriptor reports it (for drafts, before the target has
/// computed effective capabilities). No entry means the harness said nothing: unknown.
pub fn model_image_support(model: &ModelInfo) -> Support {
    model
        .capabilities
        .iter()
        .find(|c| c.capability == Capability::ImageInput)
        .map_or(Support::Unknown, |c| c.support.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_table() {
        let cases = [
            (true, Support::Supported, None, ImageRoute::AsImage),
            (true, Support::Unsupported, None, ImageRoute::AsFileCannotSee),
            (true, Support::Unsupported, Some(SendAs::Image), ImageRoute::AsFileCannotSee),
            (true, Support::Unknown, None, ImageRoute::Ask),
            (true, Support::Unknown, Some(SendAs::Image), ImageRoute::AsImage),
            (true, Support::Unknown, Some(SendAs::File), ImageRoute::AsFile),
            (true, Support::Other("maybe".into()), None, ImageRoute::Ask),
            (false, Support::Supported, None, ImageRoute::AsFile),
            (false, Support::Unsupported, None, ImageRoute::AsFile),
            (false, Support::Unknown, None, ImageRoute::AsFile),
        ];
        for (is_image, support, choice, want) in cases {
            assert_eq!(route(is_image, &support, choice), want, "{is_image} {support:?} {choice:?}");
        }
        assert!(!ImageRoute::Ask.ready());
        assert!(ImageRoute::AsFileCannotSee.ready());
    }

    #[test]
    fn model_without_modality_data_is_unknown() {
        let model = ModelInfo {
            id: "fake-model".into(),
            display_name: "fake".into(),
            description: None,
            is_default: false,
            hidden: false,
            capabilities: vec![],
            efforts: vec![],
            default_effort: None,
            context_window: None,
        };
        assert_eq!(model_image_support(&model), Support::Unknown);
    }
}
