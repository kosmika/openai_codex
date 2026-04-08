use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use libwebrtc::MediaType;
use libwebrtc::peer_connection::OfferOptions;
use libwebrtc::peer_connection::PeerConnection;
use libwebrtc::peer_connection_factory::PeerConnectionFactory;
use libwebrtc::peer_connection_factory::RtcConfiguration;
use libwebrtc::peer_connection_factory::native::PeerConnectionFactoryExt;
use libwebrtc::rtp_transceiver::RtpTransceiverDirection;
use libwebrtc::rtp_transceiver::RtpTransceiverInit;
use libwebrtc::session_description::SdpType;
use libwebrtc::session_description::SessionDescription;
use thiserror::Error;
use tracing::info;

/// Native audio media session for a realtime WebRTC call.
///
/// This owns the WebRTC peer connection and the platform audio device module. Callers create an
/// SDP offer, forward that offer through their existing signaling path, then apply the SDP answer
/// when it arrives.
pub struct RealtimeWebrtcMediaSession {
    peer_connection: PeerConnection,
    is_closed: Arc<AtomicBool>,
}

#[derive(Debug, Error)]
pub enum RealtimeWebrtcError {
    #[error("failed to create WebRTC peer connection: {0}")]
    CreatePeerConnection(String),

    #[error("failed to add WebRTC audio transceiver: {0}")]
    AddAudioTransceiver(String),

    #[error("failed to attach platform microphone to WebRTC sender: {0}")]
    AttachLocalAudio(String),

    #[error("failed to create WebRTC offer: {0}")]
    CreateOffer(String),

    #[error("failed to set local WebRTC description: {0}")]
    SetLocalDescription(String),

    #[error("failed to parse WebRTC answer SDP: {0}")]
    ParseAnswer(String),

    #[error("failed to set remote WebRTC description: {0}")]
    SetRemoteDescription(String),
}

impl RealtimeWebrtcMediaSession {
    pub async fn create_offer() -> Result<(Self, String), RealtimeWebrtcError> {
        info!("initializing realtime WebRTC media session");
        let factory = PeerConnectionFactory::with_platform_adm();
        let peer_connection = factory
            .create_peer_connection(RtcConfiguration::default())
            .map_err(|err| RealtimeWebrtcError::CreatePeerConnection(err.to_string()))?;

        let audio_transceiver = peer_connection
            .add_transceiver_for_media(
                MediaType::Audio,
                RtpTransceiverInit {
                    direction: RtpTransceiverDirection::SendRecv,
                    stream_ids: vec!["realtime".to_string()],
                    send_encodings: Vec::new(),
                },
            )
            .map_err(|err| RealtimeWebrtcError::AddAudioTransceiver(err.to_string()))?;

        let local_audio_source = factory.create_audio_source();
        let local_audio_track = factory.create_audio_track("realtime-mic", local_audio_source);
        audio_transceiver
            .sender()
            .set_track(Some(local_audio_track.into()))
            .map_err(|err| RealtimeWebrtcError::AttachLocalAudio(err.to_string()))?;

        let offer = peer_connection
            .create_offer(OfferOptions {
                ice_restart: false,
                offer_to_receive_audio: true,
                offer_to_receive_video: false,
            })
            .await
            .map_err(|err| RealtimeWebrtcError::CreateOffer(err.to_string()))?;
        peer_connection
            .set_local_description(offer.clone())
            .await
            .map_err(|err| RealtimeWebrtcError::SetLocalDescription(err.to_string()))?;

        let session = Self {
            peer_connection,
            is_closed: Arc::new(AtomicBool::new(false)),
        };
        Ok((session, offer.to_string()))
    }

    pub async fn accept_answer(&self, sdp: &str) -> Result<(), RealtimeWebrtcError> {
        let answer = SessionDescription::parse(sdp, SdpType::Answer)
            .map_err(|err| RealtimeWebrtcError::ParseAnswer(err.to_string()))?;
        self.peer_connection
            .set_remote_description(answer)
            .await
            .map_err(|err| RealtimeWebrtcError::SetRemoteDescription(err.to_string()))
    }

    pub fn close(&self) {
        if !self.is_closed.swap(true, Ordering::SeqCst) {
            self.peer_connection.close();
        }
    }
}

impl Drop for RealtimeWebrtcMediaSession {
    fn drop(&mut self) {
        self.close();
    }
}
