//! The dedicated call renderer. Only the bundled page in this isolated context can use audio.
use super::*;
use wire::CallEvent;

const ORIGIN: &str = "https://yaac-call.invalid";
pub(super) const URL: &str = "https://yaac-call.invalid/call";
const PAGE: &[u8] = include_bytes!("voice_call.html");
const MAX_MESSAGE: usize = 128 * 1024;

wrap_app! {
    pub(super) struct CallApp;
    impl App {
        fn on_before_command_line_processing(&self, _process: Option<&CefString>, command: Option<&mut CommandLine>) {
            if let Some(command) = command {
                // There is no input surface for the offscreen audio element.
                command.append_switch_with_value(Some(&"autoplay-policy".into()), Some(&"no-user-gesture-required".into()));
                command.append_switch(Some(&"disable-background-networking".into()));
                command.append_switch(Some(&"disable-component-update".into()));
            }
        }
    }
}

wrap_client! {
    pub(super) struct CallPage { s: Shared<State> }
    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(Render::new(self.s.clone()))
        }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(Life::new(self.s.clone()))
        }
        fn request_handler(&self) -> Option<RequestHandler> {
            Some(Navigation::new(self.s.id))
        }
        fn permission_handler(&self) -> Option<PermissionHandler> {
            Some(Microphone::new())
        }
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(Events::new(self.s.id))
        }
    }
}

wrap_permission_handler! {
    struct Microphone;
    impl PermissionHandler {
        fn on_request_media_access_permission(
            &self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>,
            origin: Option<&CefString>, requested: u32, callback: Option<&mut MediaAccessCallback>,
        ) -> i32 {
            let audio = *MediaAccessPermissionTypes::DEVICE_AUDIO_CAPTURE.as_ref() as u32;
            let approved = requested == audio
                && origin.is_some_and(|origin| origin.to_string().trim_end_matches('/') == ORIGIN)
                && frame.is_some_and(|frame| frame.is_main() != 0 && CefString::from(&frame.url()).to_string() == URL);
            if let Some(callback) = callback {
                callback.cont(if approved { audio } else { 0 });
            }
            1
        }
    }
}

wrap_request_handler! {
    struct Navigation { id: u64 }
    impl RequestHandler {
        fn on_before_browse(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>,
            request: Option<&mut cef::Request>, _gesture: i32, redirect: i32) -> i32 {
            i32::from(redirect != 0
                || !frame.is_some_and(|frame| frame.is_main() != 0)
                || !request.is_some_and(|request| CefString::from(&request.url()).to_string() == URL))
        }
        fn on_render_process_terminated(&self, _browser: Option<&mut Browser>, _status: TerminationStatus,
            _code: i32, _message: Option<&CefString>) {
            emit(self.id, Event::Call(CallEvent::Failed("The call page stopped.".into())));
        }
    }
}

wrap_display_handler! {
    struct Events { id: u64 }
    impl DisplayHandler {
        fn on_console_message(&self, browser: Option<&mut Browser>, _level: LogSeverity,
            message: Option<&CefString>, source: Option<&CefString>, _line: i32) -> i32 {
            if source.is_some_and(|source| source.to_string() == URL)
                && browser.and_then(|browser| browser.main_frame()).is_some_and(|frame| CefString::from(&frame.url()).to_string() == URL)
                && let Some(message) = message
            {
                let message = message.to_string();
                if let Some(body) = message.strip_prefix("yaac-call:")
                    && body.len() <= MAX_MESSAGE
                    && let Ok(event) = serde_json::from_str::<CallEvent>(body)
                {
                    emit(self.id, Event::Call(event));
                }
            }
            // The offer is a private pipe message, never a Chromium console log.
            1
        }
    }
}

wrap_scheme_handler_factory! {
    struct CallResource;
    impl SchemeHandlerFactory {
        fn create(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>,
            _scheme: Option<&CefString>, request: Option<&mut cef::Request>) -> Option<ResourceHandler> {
            use cef::wrapper::stream_resource_handler::StreamResourceHandler;
            if CefString::from(&request?.url()).to_string() != URL {
                return Some(StreamResourceHandler::new(404, "Not Found".into(), "text/plain".into(), None, None));
            }
            let stream = stream_reader_create_for_data(PAGE.as_ptr().cast_mut(), PAGE.len())?;
            Some(StreamResourceHandler::new_with_stream("text/html".into(), stream))
        }
    }
}

pub(super) fn register(ctx: &RequestContext) -> bool {
    ctx.register_scheme_handler_factory(
        Some(&"https".into()),
        Some(&"yaac-call.invalid".into()),
        Some(&mut CallResource::new()),
    ) != 0
}

pub(super) fn control(browser: &Browser, action: Action) {
    let script = match action {
        Action::CallAnswer(sdp) if sdp.len() <= MAX_MESSAGE => {
            let Ok(sdp) = serde_json::to_string(&sdp) else {
                return;
            };
            format!("window.yaacCall.answer({sdp})")
        }
        Action::CallMute(muted) => format!("window.yaacCall.mute({muted})"),
        _ => return,
    };
    if let Some(frame) = browser.main_frame()
        && CefString::from(&frame.url()).to_string() == URL
    {
        frame.execute_java_script(Some(&script.as_str().into()), Some(&URL.into()), 0);
    }
}
