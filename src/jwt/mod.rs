use crate::{
    JsonWebKey, JsonWebKeyId, JsonWebKeyType, JsonWebKeyUse, JweContentEncryptionAlgorithm,
    JwsSigningAlgorithm, PrivateSigningKey, SignatureVerificationError, SigningError,
};

use serde::de::{DeserializeOwned, Error as _, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use std::fmt::Debug;
use std::marker::PhantomData;

#[cfg(test)]
pub(crate) mod tests;

new_type![
    #[derive(Deserialize, Hash, Ord, PartialOrd, Serialize)]
    JsonWebTokenContentType(String)
];

new_type![
    #[derive(Deserialize, Hash, Ord, PartialOrd, Serialize)]
    JsonWebTokenType(String)
];

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum JsonWebTokenAlgorithm<JE, JS, JT>
where
    JE: JweContentEncryptionAlgorithm<JT>,
    JS: JwsSigningAlgorithm<JT>,
    JT: JsonWebKeyType,
{
    Encryption(JE),
    // This is ugly, but we don't expose this module via the public API, so it's fine.
    Signature(JS, PhantomData<JT>),
    /// No digital signature or MAC performed.
    ///
    /// # Security Warning
    ///
    /// This algorithm provides no security over the integrity of the JSON Web Token. Clients
    /// should be careful not to rely on unsigned JWT's for security purposes. See
    /// [Critical vulnerabilities in JSON Web Token libraries](
    ///     https://auth0.com/blog/critical-vulnerabilities-in-json-web-token-libraries/) for
    /// further discussion.
    None,
}
impl<'de, JE, JS, JT> Deserialize<'de> for JsonWebTokenAlgorithm<JE, JS, JT>
where
    JE: JweContentEncryptionAlgorithm<JT>,
    JS: JwsSigningAlgorithm<JT>,
    JT: JsonWebKeyType,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value: serde_json::Value = Deserialize::deserialize(deserializer)?;
        // TODO: get rid of this clone() (see below)
        let s: String = serde_json::from_value(value.clone()).map_err(D::Error::custom)?;

        // NB: These comparisons are case-sensitive. Section 4.1.1 of RFC 7515 states: "The "alg"
        // value is a case-sensitive ASCII string containing a StringOrURI value."
        if s == "none" {
            Ok(JsonWebTokenAlgorithm::None)
        // TODO: Figure out a way to deserialize the enums without giving up ownership
        } else if let Ok(val) = serde_json::from_value::<JE>(value.clone()) {
            Ok(JsonWebTokenAlgorithm::Encryption(val))
        } else if let Ok(val) = serde_json::from_value::<JS>(value) {
            Ok(JsonWebTokenAlgorithm::Signature(val, PhantomData))
        } else {
            Err(D::Error::custom(format!(
                "unrecognized JSON Web Algorithm `{}`",
                s
            )))
        }
    }
}
impl<JE, JS, JT> Serialize for JsonWebTokenAlgorithm<JE, JS, JT>
where
    JE: JweContentEncryptionAlgorithm<JT>,
    JS: JwsSigningAlgorithm<JT>,
    JT: JsonWebKeyType,
{
    fn serialize<SE>(&self, serializer: SE) -> Result<SE::Ok, SE::Error>
    where
        SE: Serializer,
    {
        match self {
            JsonWebTokenAlgorithm::Encryption(ref enc) => enc.serialize(serializer),
            JsonWebTokenAlgorithm::Signature(ref sig, _) => sig.serialize(serializer),
            JsonWebTokenAlgorithm::None => serializer.serialize_str("none"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct JsonWebTokenHeader<JE, JS, JT>
where
    JE: JweContentEncryptionAlgorithm<JT>,
    JS: JwsSigningAlgorithm<JT>,
    JT: JsonWebKeyType,
{
    #[serde(
        bound = "JE: JweContentEncryptionAlgorithm<JT>, JS: JwsSigningAlgorithm<JT>, JT: JsonWebKeyType"
    )]
    pub alg: JsonWebTokenAlgorithm<JE, JS, JT>,
    // Additional critical header parameters that must be understood by this implementation. Since
    // we don't understand any such extensions, we reject any JWT with this value present (the
    // spec specifically prohibits including public (standard) headers in this field).
    // See https://tools.ietf.org/html/rfc7515#section-4.1.11.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crit: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cty: Option<JsonWebTokenContentType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kid: Option<JsonWebKeyId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typ: Option<JsonWebTokenType>,
    // Other JOSE header fields are omitted since the OpenID Connect spec specifically says that
    // the "x5u", "x5c", "jku", "jwk" header parameter fields SHOULD NOT be used.
    // See http://openid.net/specs/openid-connect-core-1_0-final.html#IDToken.
    #[serde(skip)]
    _phantom_jt: PhantomData<JT>,
}

pub trait JsonWebTokenPayloadSerde<P>: Debug
where
    P: Debug + DeserializeOwned + Serialize,
{
    fn deserialize<DE: serde::de::Error>(payload: &[u8]) -> Result<P, DE>;
    fn serialize(payload: &P) -> Result<String, serde_json::Error>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonWebTokenJsonPayloadSerde;
impl<P> JsonWebTokenPayloadSerde<P> for JsonWebTokenJsonPayloadSerde
where
    P: Debug + DeserializeOwned + Serialize,
{
    fn deserialize<DE: serde::de::Error>(payload: &[u8]) -> Result<P, DE> {
        serde_json::from_slice(payload)
            .map_err(|err| DE::custom(format!("Failed to parse payload JSON: {:?}", err)))
    }

    fn serialize(payload: &P) -> Result<String, serde_json::Error> {
        serde_json::to_string(payload).map_err(Into::into)
    }
}

// Helper trait so that we can get borrowed payload when we have a reference to the JWT and owned
// payload when we own the JWT.
pub trait JsonWebTokenAccess<JE, JS, JT, P>
where
    JE: JweContentEncryptionAlgorithm<JT>,
    JS: JwsSigningAlgorithm<JT>,
    JT: JsonWebKeyType,
    P: Debug + DeserializeOwned + Serialize,
{
    type ReturnType;

    fn unverified_header(&self) -> &JsonWebTokenHeader<JE, JS, JT>;
    fn unverified_payload(self) -> Self::ReturnType;
    fn unverified_payload_ref(&self) -> &P;

    fn payload<JU, JW>(
        self,
        signature_alg: &JS,
        key: &JW,
    ) -> Result<Self::ReturnType, SignatureVerificationError>
    where
        JU: JsonWebKeyUse,
        JW: JsonWebKey<JS, JT, JU>;
}

/// Error creating a JSON Web Token.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum JsonWebTokenError {
    /// Failed to serialize JWT.
    #[error("Failed to serialize JWT")]
    SerializationError(#[source] serde_json::Error),
    /// Failed to sign JWT.
    #[error("Failed to sign JWT")]
    SigningError(#[source] SigningError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonWebToken<JE, JS, JT, P, S>
where
    JE: JweContentEncryptionAlgorithm<JT>,
    JS: JwsSigningAlgorithm<JT>,
    JT: JsonWebKeyType,
    P: Debug + DeserializeOwned + Serialize,
    S: JsonWebTokenPayloadSerde<P>,
{
    header: JsonWebTokenHeader<JE, JS, JT>,
    payload: P,
    signature: Vec<u8>,
    signing_input: String,
    _phantom: PhantomData<S>,
}
impl<JE, JS, JT, P, S> JsonWebToken<JE, JS, JT, P, S>
where
    JE: JweContentEncryptionAlgorithm<JT>,
    JS: JwsSigningAlgorithm<JT>,
    JT: JsonWebKeyType,
    P: Debug + DeserializeOwned + Serialize,
    S: JsonWebTokenPayloadSerde<P>,
{
    pub fn new<JU, K, SK>(payload: P, signing_key: &SK, alg: &JS) -> Result<Self, JsonWebTokenError>
    where
        JU: JsonWebKeyUse,
        K: JsonWebKey<JS, JT, JU>,
        SK: PrivateSigningKey<JS, JT, JU, K>,
    {
        let header = JsonWebTokenHeader::<JE, _, _> {
            alg: JsonWebTokenAlgorithm::Signature(alg.clone(), PhantomData),
            crit: None,
            cty: None,
            kid: signing_key.as_verification_key().key_id().cloned(),
            typ: None,
            _phantom_jt: PhantomData,
        };

        let header_json =
            serde_json::to_string(&header).map_err(JsonWebTokenError::SerializationError)?;
        let header_base64 = base64::encode_config(header_json, base64::URL_SAFE_NO_PAD);

        let serialized_payload =
            S::serialize(&payload).map_err(JsonWebTokenError::SerializationError)?;
        let payload_base64 = base64::encode_config(serialized_payload, base64::URL_SAFE_NO_PAD);

        let signing_input = format!("{}.{}", header_base64, payload_base64);

        let signature = signing_key
            .sign(alg, signing_input.as_bytes())
            .map_err(JsonWebTokenError::SigningError)?;

        Ok(JsonWebToken {
            header,
            payload,
            signature,
            signing_input,
            _phantom: PhantomData,
        })
    }
}
// Owned JWT.
impl<JE, JS, JT, P, S> JsonWebTokenAccess<JE, JS, JT, P> for JsonWebToken<JE, JS, JT, P, S>
where
    JE: JweContentEncryptionAlgorithm<JT>,
    JS: JwsSigningAlgorithm<JT>,
    JT: JsonWebKeyType,
    P: Debug + DeserializeOwned + Serialize,
    S: JsonWebTokenPayloadSerde<P>,
{
    type ReturnType = P;
    fn unverified_header(&self) -> &JsonWebTokenHeader<JE, JS, JT> {
        &self.header
    }
    fn unverified_payload(self) -> Self::ReturnType {
        self.payload
    }
    fn unverified_payload_ref(&self) -> &P {
        &self.payload
    }
    fn payload<JU, JW>(
        self,
        signature_alg: &JS,
        key: &JW,
    ) -> Result<Self::ReturnType, SignatureVerificationError>
    where
        JU: JsonWebKeyUse,
        JW: JsonWebKey<JS, JT, JU>,
    {
        key.verify_signature(
            signature_alg,
            self.signing_input.as_bytes(),
            &self.signature,
        )?;
        Ok(self.payload)
    }
}
// Borrowed JWT.
impl<'a, JE, JS, JT, P, S> JsonWebTokenAccess<JE, JS, JT, P> for &'a JsonWebToken<JE, JS, JT, P, S>
where
    JE: JweContentEncryptionAlgorithm<JT>,
    JS: JwsSigningAlgorithm<JT>,
    JT: JsonWebKeyType,
    P: Debug + DeserializeOwned + Serialize,
    S: JsonWebTokenPayloadSerde<P>,
{
    type ReturnType = &'a P;
    fn unverified_header(&self) -> &JsonWebTokenHeader<JE, JS, JT> {
        &self.header
    }
    fn unverified_payload(self) -> Self::ReturnType {
        &self.payload
    }
    fn unverified_payload_ref(&self) -> &P {
        &self.payload
    }
    fn payload<JU, JW>(
        self,
        signature_alg: &JS,
        key: &JW,
    ) -> Result<Self::ReturnType, SignatureVerificationError>
    where
        JU: JsonWebKeyUse,
        JW: JsonWebKey<JS, JT, JU>,
    {
        key.verify_signature(
            signature_alg,
            self.signing_input.as_bytes(),
            &self.signature,
        )?;
        Ok(&self.payload)
    }
}
impl<'de, JE, JS, JT, P, S> Deserialize<'de> for JsonWebToken<JE, JS, JT, P, S>
where
    JE: JweContentEncryptionAlgorithm<JT>,
    JS: JwsSigningAlgorithm<JT>,
    JT: JsonWebKeyType,
    P: Debug + DeserializeOwned + Serialize,
    S: JsonWebTokenPayloadSerde<P>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct JsonWebTokenVisitor<
            JE: JweContentEncryptionAlgorithm<JT>,
            JS: JwsSigningAlgorithm<JT>,
            JT: JsonWebKeyType,
            P: Debug + DeserializeOwned + Serialize,
            S: JsonWebTokenPayloadSerde<P>,
        >(
            PhantomData<JE>,
            PhantomData<JS>,
            PhantomData<JT>,
            PhantomData<P>,
            PhantomData<S>,
        );
        impl<'de, JE, JS, JT, P, S> Visitor<'de> for JsonWebTokenVisitor<JE, JS, JT, P, S>
        where
            JE: JweContentEncryptionAlgorithm<JT>,
            JS: JwsSigningAlgorithm<JT>,
            JT: JsonWebKeyType,
            P: Debug + DeserializeOwned + Serialize,
            S: JsonWebTokenPayloadSerde<P>,
        {
            type Value = JsonWebToken<JE, JS, JT, P, S>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("JsonWebToken")
            }

            fn visit_str<DE>(self, v: &str) -> Result<Self::Value, DE>
            where
                DE: serde::de::Error,
            {
                let raw_token = v.to_string();
                let header: JsonWebTokenHeader<JE, JS, JT>;
                let payload: P;
                let signature;
                let signing_input;

                {
                    let parts = raw_token.split('.').collect::<Vec<_>>();

                    // NB: We avoid including the full payload encoding in the error output to avoid
                    // clients potentially logging sensitive values.
                    if parts.len() != 3 {
                        return Err(DE::custom(format!(
                            "Invalid JSON web token: found {} parts (expected 3)",
                            parts.len()
                        )));
                    }

                    let header_json =
                        base64::decode_config(parts[0], crate::core::base64_url_safe_no_pad())
                            .map_err(|err| {
                                DE::custom(format!("Invalid base64url header encoding: {:?}", err))
                            })?;
                    header = serde_json::from_slice(&header_json).map_err(|err| {
                        DE::custom(format!("Failed to parse header JSON: {:?}", err))
                    })?;

                    let raw_payload =
                        base64::decode_config(parts[1], crate::core::base64_url_safe_no_pad())
                            .map_err(|err| {
                                DE::custom(format!("Invalid base64url payload encoding: {:?}", err))
                            })?;
                    payload = S::deserialize::<DE>(&raw_payload)?;

                    signature =
                        base64::decode_config(parts[2], crate::core::base64_url_safe_no_pad())
                            .map_err(|err| {
                                DE::custom(format!(
                                    "Invalid base64url signature encoding: {:?}",
                                    err
                                ))
                            })?;

                    signing_input = format!("{}.{}", parts[0], parts[1]);
                }

                Ok(JsonWebToken {
                    header,
                    payload,
                    signature,
                    signing_input,
                    _phantom: PhantomData,
                })
            }
        }
        deserializer.deserialize_str(JsonWebTokenVisitor(
            PhantomData,
            PhantomData,
            PhantomData,
            PhantomData,
            PhantomData,
        ))
    }
}
impl<JE, JS, JT, P, S> Serialize for JsonWebToken<JE, JS, JT, P, S>
where
    JE: JweContentEncryptionAlgorithm<JT>,
    JS: JwsSigningAlgorithm<JT>,
    JT: JsonWebKeyType,
    P: Debug + DeserializeOwned + Serialize,
    S: JsonWebTokenPayloadSerde<P>,
{
    fn serialize<SE>(&self, serializer: SE) -> Result<SE::Ok, SE::Error>
    where
        SE: Serializer,
    {
        let signature_base64 = base64::encode_config(&self.signature, base64::URL_SAFE_NO_PAD);
        serializer.serialize_str(&format!("{}.{}", self.signing_input, signature_base64))
    }
}
