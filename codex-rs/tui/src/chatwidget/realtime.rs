use super::*;
use codex_protocol::protocol::ConversationStartParams;
use codex_protocol::protocol::RealtimeAudioFrame;
use codex_protocol::protocol::RealtimeConversationClosedEvent;
use codex_protocol::protocol::RealtimeConversationRealtimeEvent;
use codex_protocol::protocol::RealtimeConversationStartedEvent;
use codex_protocol::protocol::RealtimeEvent;

const REALTIME_CONVERSATION_PROMPT: &str = "You are in a realtime voice conversation in the Codex TUI. Respond conversationally and concisely.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum RealtimeConversationPhase {
    #[default]
    Inactive,
    Starting,
    Active,
    Stopping,
}

#[derive(Default)]
pub(super) struct RealtimeConversationUiState {
    pub(super) phase: RealtimeConversationPhase,
    requested_close: bool,
    session_id: Option<String>,
    warned_audio_only_submission: bool,
}

impl RealtimeConversationUiState {
    pub(super) fn is_live(&self) -> bool {
        matches!(
            self.phase,
            RealtimeConversationPhase::Starting
                | RealtimeConversationPhase::Active
                | RealtimeConversationPhase::Stopping
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct RenderedUserMessageEvent {
    pub(super) message: String,
    pub(super) remote_image_urls: Vec<String>,
    pub(super) local_images: Vec<PathBuf>,
    pub(super) text_elements: Vec<TextElement>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PendingSteerCompareKey {
    pub(super) message: String,
    pub(super) image_count: usize,
}

impl ChatWidget {
    pub(super) fn rendered_user_message_event_from_parts(
        message: String,
        text_elements: Vec<TextElement>,
        local_images: Vec<PathBuf>,
        remote_image_urls: Vec<String>,
    ) -> RenderedUserMessageEvent {
        RenderedUserMessageEvent {
            message,
            remote_image_urls,
            local_images,
            text_elements,
        }
    }

    pub(super) fn rendered_user_message_event_from_event(
        event: &UserMessageEvent,
    ) -> RenderedUserMessageEvent {
        Self::rendered_user_message_event_from_parts(
            event.message.clone(),
            event.text_elements.clone(),
            event.local_images.clone(),
            event.images.clone().unwrap_or_default(),
        )
    }

    /// Build the compare key for a submitted pending steer without invoking the
    /// expensive request-serialization path. Pending steers only need to match the
    /// committed `ItemCompleted(UserMessage)` emitted after core drains input, which
    /// preserves flattened text and total image count but not UI-only text ranges or
    /// local image paths.
    pub(super) fn pending_steer_compare_key_from_items(
        items: &[UserInput],
    ) -> PendingSteerCompareKey {
        let mut message = String::new();
        let mut image_count = 0;

        for item in items {
            match item {
                UserInput::Text { text, .. } => message.push_str(text),
                UserInput::Image { .. } | UserInput::LocalImage { .. } => image_count += 1,
                UserInput::Skill { .. } | UserInput::Mention { .. } => {}
                _ => {}
            }
        }

        PendingSteerCompareKey {
            message,
            image_count,
        }
    }

    #[cfg(test)]
    pub(super) fn pending_steer_compare_key_from_item(
        item: &codex_protocol::items::UserMessageItem,
    ) -> PendingSteerCompareKey {
        Self::pending_steer_compare_key_from_items(&item.content)
    }

    #[cfg(test)]
    pub(super) fn rendered_user_message_event_from_inputs(
        items: &[UserInput],
    ) -> RenderedUserMessageEvent {
        let mut message = String::new();
        let mut remote_image_urls = Vec::new();
        let mut local_images = Vec::new();
        let mut text_elements = Vec::new();

        for item in items {
            match item {
                UserInput::Text {
                    text,
                    text_elements: current_text_elements,
                } => append_text_with_rebased_elements(
                    &mut message,
                    &mut text_elements,
                    text,
                    current_text_elements.iter().map(|element| {
                        TextElement::new(
                            element.byte_range,
                            element.placeholder(text).map(str::to_string),
                        )
                    }),
                ),
                UserInput::Image { image_url } => remote_image_urls.push(image_url.clone()),
                UserInput::LocalImage { path } => local_images.push(path.clone()),
                UserInput::Skill { .. } | UserInput::Mention { .. } => {}
                _ => {}
            }
        }

        Self::rendered_user_message_event_from_parts(
            message,
            text_elements,
            local_images,
            remote_image_urls,
        )
    }

    #[cfg(test)]
    pub(super) fn should_render_realtime_user_message_event(
        &self,
        event: &UserMessageEvent,
    ) -> bool {
        if !self.realtime_conversation.is_live() {
            return false;
        }
        let key = Self::rendered_user_message_event_from_event(event);
        self.last_rendered_user_message_event.as_ref() != Some(&key)
    }

    pub(super) fn maybe_defer_user_message_for_realtime(
        &mut self,
        user_message: UserMessage,
    ) -> Option<UserMessage> {
        if !self.realtime_conversation.is_live() {
            return Some(user_message);
        }

        self.restore_user_message_to_composer(user_message);
        if !self.realtime_conversation.warned_audio_only_submission {
            self.realtime_conversation.warned_audio_only_submission = true;
            self.add_info_message(
                "Realtime voice mode is audio-only. Use /realtime to stop.".to_string(),
                /*hint*/ None,
            );
        } else {
            self.request_redraw();
        }

        None
    }

    fn realtime_footer_hint_items() -> Vec<(String, String)> {
        vec![("/realtime".to_string(), "stop live voice".to_string())]
    }

    pub(super) fn stop_realtime_conversation_from_ui(&mut self) {
        self.request_realtime_conversation_close(/*info_message*/ None);
    }

    pub(super) fn start_realtime_conversation(&mut self) {
        self.realtime_conversation.phase = RealtimeConversationPhase::Starting;
        self.realtime_conversation.requested_close = false;
        self.realtime_conversation.session_id = None;
        self.realtime_conversation.warned_audio_only_submission = false;
        self.set_footer_hint_override(Some(Self::realtime_footer_hint_items()));
        self.submit_op(AppCommand::realtime_conversation_start(
            ConversationStartParams {
                prompt: REALTIME_CONVERSATION_PROMPT.to_string(),
                session_id: None,
                transport: None,
            },
        ));
        self.request_redraw();
    }

    pub(super) fn request_realtime_conversation_close(&mut self, info_message: Option<String>) {
        if !self.realtime_conversation.is_live() {
            if let Some(message) = info_message {
                self.add_info_message(message, /*hint*/ None);
            }
            return;
        }

        self.realtime_conversation.requested_close = true;
        self.realtime_conversation.phase = RealtimeConversationPhase::Stopping;
        self.submit_op(AppCommand::realtime_conversation_close());
        self.stop_realtime_local_audio();
        self.set_footer_hint_override(/*items*/ None);

        if let Some(message) = info_message {
            self.add_info_message(message, /*hint*/ None);
        } else {
            self.request_redraw();
        }
    }

    pub(super) fn reset_realtime_conversation_state(&mut self) {
        self.stop_realtime_local_audio();
        self.set_footer_hint_override(/*items*/ None);
        self.realtime_conversation.phase = RealtimeConversationPhase::Inactive;
        self.realtime_conversation.requested_close = false;
        self.realtime_conversation.session_id = None;
        self.realtime_conversation.warned_audio_only_submission = false;
    }

    fn fail_realtime_conversation(&mut self, message: String) {
        self.add_error_message(message);
        if self.realtime_conversation.is_live() {
            self.request_realtime_conversation_close(/*info_message*/ None);
        } else {
            self.reset_realtime_conversation_state();
            self.request_redraw();
        }
    }

    pub(super) fn on_realtime_conversation_started(
        &mut self,
        ev: RealtimeConversationStartedEvent,
    ) {
        if !self.realtime_conversation_enabled() {
            self.request_realtime_conversation_close(/*info_message*/ None);
            return;
        }
        self.realtime_conversation.phase = RealtimeConversationPhase::Active;
        self.realtime_conversation.session_id = ev.session_id;
        self.realtime_conversation.warned_audio_only_submission = false;
        self.set_footer_hint_override(Some(Self::realtime_footer_hint_items()));
        self.start_realtime_local_audio();
        self.request_redraw();
    }

    pub(super) fn on_realtime_conversation_realtime(
        &mut self,
        ev: RealtimeConversationRealtimeEvent,
    ) {
        match ev.payload {
            RealtimeEvent::SessionUpdated { session_id, .. } => {
                self.realtime_conversation.session_id = Some(session_id);
            }
            RealtimeEvent::InputAudioSpeechStarted(_) => self.interrupt_realtime_audio_playback(),
            RealtimeEvent::InputTranscriptDelta(_) => {}
            RealtimeEvent::OutputTranscriptDelta(_) => {}
            RealtimeEvent::AudioOut(frame) => self.enqueue_realtime_audio_out(&frame),
            RealtimeEvent::ResponseCancelled(_) => self.interrupt_realtime_audio_playback(),
            RealtimeEvent::ConversationItemAdded(_item) => {}
            RealtimeEvent::ConversationItemDone { .. } => {}
            RealtimeEvent::HandoffRequested(_) => {}
            RealtimeEvent::Error(message) => {
                self.fail_realtime_conversation(format!("Realtime voice error: {message}"));
            }
        }
    }

    pub(super) fn on_realtime_conversation_closed(&mut self, ev: RealtimeConversationClosedEvent) {
        let requested = self.realtime_conversation.requested_close;
        let reason = ev.reason;
        if self.realtime_webrtc_media_enabled()
            && !requested
            && reason.as_deref() == Some("transport_closed")
        {
            self.request_redraw();
            return;
        }

        if self.realtime_webrtc_media_enabled() {
            self.app_event_tx.send(AppEvent::RealtimeWebrtcClose);
        }
        self.reset_realtime_conversation_state();
        if !requested
            && let Some(reason) = reason
            && reason != "error"
        {
            self.add_info_message(
                format!("Realtime voice mode closed: {reason}"),
                /*hint*/ None,
            );
        }
        self.request_redraw();
    }

    fn enqueue_realtime_audio_out(&mut self, frame: &RealtimeAudioFrame) {
        let _ = frame;
    }

    fn interrupt_realtime_audio_playback(&mut self) {
        // Native realtime media transport owns interruption on the audio path.
    }

    fn start_realtime_local_audio(&mut self) {
        self.request_redraw();
    }

    pub(crate) fn restart_realtime_audio_device(&mut self, kind: RealtimeAudioDeviceKind) {
        let _ = kind;
    }

    fn stop_realtime_local_audio(&mut self) {}
}
