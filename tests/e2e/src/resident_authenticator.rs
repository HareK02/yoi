// 共有テスト用のresident認証器モデル。ブラウザ／OSの実装ではない。
// SoftPasskey 0.5.5自体はRK要求を拒否しdiscoverできないため、このモデルが
// RP・credential ID・user handleのresident storeと明示選択を所有し、
// SoftPasskeyはattestation／署名エンジンとしてだけ使う。本番verifier、
// challenge、session発行を固定成功fakeへ置換しない。UVはテスト用bitの
// 模擬であり、物理的PIN／生体確認やbrowser-nativeの互換性は未検証。
use std::collections::BTreeMap;
use webauthn_authenticator_rs::prelude::{Url, WebauthnCError};
use webauthn_authenticator_rs::{WebauthnAuthenticator, softpasskey::SoftPasskey};
use webauthn_rs_proto::{
    AllowCredentials, CreationChallengeResponse, PublicKeyCredential, RegisterPublicKeyCredential,
    RequestChallengeResponse, ResidentKeyRequirement,
};

pub struct ResidentAuthenticator {
    signer: WebauthnAuthenticator<SoftPasskey>,
    resident: BTreeMap<Vec<u8>, (String, Vec<u8>)>,
    selected: Option<Vec<u8>>,
}

impl ResidentAuthenticator {
    pub fn new(simulate_uv: bool) -> Self {
        Self {
            signer: WebauthnAuthenticator::new(SoftPasskey::new(simulate_uv)),
            resident: BTreeMap::new(),
            selected: None,
        }
    }

    pub fn select(&mut self, credential_id: &[u8]) {
        assert!(
            self.resident.contains_key(credential_id),
            "selection must be resident on this authenticator"
        );
        self.selected = Some(credential_id.to_vec());
    }

    pub fn do_registration(
        &mut self,
        origin: Url,
        mut options: CreationChallengeResponse,
    ) -> Result<RegisterPublicKeyCredential, WebauthnCError> {
        let rp = options.public_key.rp.id.clone();
        let user = options.public_key.user.id.to_vec();
        let resident = options
            .public_key
            .authenticator_selection
            .as_ref()
            .is_some_and(|selection| {
                selection.require_resident_key
                    || selection.resident_key == Some(ResidentKeyRequirement::Required)
            });
        if resident {
            // 要求されたresident保存はこのモデルが担当し、署名だけを
            // non-residentエンジンへ渡す。Serverの要求を黙って無視しない。
            let selection = options.public_key.authenticator_selection.as_mut().unwrap();
            selection.require_resident_key = false;
            selection.resident_key = Some(ResidentKeyRequirement::Discouraged);
        }
        let credential = self.signer.do_registration(origin, options)?;
        if resident {
            self.resident.insert(credential.raw_id.to_vec(), (rp, user));
            self.selected = None;
        }
        Ok(credential)
    }

    pub fn do_authentication(
        &mut self,
        origin: Url,
        mut options: RequestChallengeResponse,
    ) -> Result<PublicKeyCredential, WebauthnCError> {
        let discovered = options.public_key.allow_credentials.is_empty();
        let user_handle = if discovered {
            let candidates: Vec<_> = self
                .resident
                .iter()
                .filter(|(_, (rp, _))| rp == &options.public_key.rp_id)
                .collect();
            let (id, (_, handle)) = match self.selected.as_ref() {
                Some(selected) => candidates.into_iter().find(|(id, _)| *id == selected),
                None if candidates.len() == 1 => candidates.into_iter().next(),
                _ => None,
            }
            .ok_or(WebauthnCError::NotSupported)?;
            options.public_key.allow_credentials = vec![AllowCredentials {
                type_: "public-key".into(),
                id: id.clone().into(),
                transports: None,
            }];
            Some(handle.clone())
        } else {
            None
        };
        let mut credential = self.signer.do_authentication(origin, options)?;
        credential.response.user_handle = user_handle.map(Into::into);
        Ok(credential)
    }
}
