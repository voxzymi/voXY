//! The sandbox wire protocol (design §6.7, owner decision D3): versioned
//! `Request`/`Reply` messages in length-prefixed `postcard` frames.
//!
//! A frame is a little-endian `u32` payload length followed by the payload.
//! Frames over [`MAX_FRAME`] are refused before anything is allocated, and
//! a payload that does not decode exactly is an error, never a panic: the
//! editor reads frames written by a process that runs third-party code.

use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::effect::{Message, Project};
use crate::image::{Components, Depth, Premult};
use crate::params::{ParamKind, ParamValue};
use crate::props::{PropSet, Values};

/// Bumped on any incompatible change; `Hello` checks it.
pub const VERSION: u32 = 1;

/// The largest payload accepted (images travel in shared memory, so frames
/// stay small).
pub const MAX_FRAME: u32 = 16 << 20;

/// Why a frame could not be read.
#[derive(Debug)]
pub enum FrameError {
    /// The stream failed or ended.
    Io(io::Error),
    /// The length prefix is over [`MAX_FRAME`].
    TooLarge(u32),
    /// The payload is not a valid message.
    Decode(postcard::Error),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::Io(e) => write!(f, "channel error: {e}"),
            FrameError::TooLarge(n) => write!(f, "frame of {n} bytes is over the limit"),
            FrameError::Decode(e) => write!(f, "malformed frame: {e}"),
        }
    }
}

impl std::error::Error for FrameError {}

impl From<io::Error> for FrameError {
    fn from(e: io::Error) -> Self {
        FrameError::Io(e)
    }
}

/// Writes one message as a frame.
pub fn write_frame<T: Serialize>(w: &mut impl Write, msg: &T) -> io::Result<()> {
    let payload = postcard::to_stdvec(msg).map_err(io::Error::other)?;
    let len = u32::try_from(payload.len())
        .ok()
        .filter(|n| *n <= MAX_FRAME)
        .ok_or_else(|| io::Error::other("message too large for one frame"))?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(&payload)?;
    w.flush()
}

/// Reads one frame and decodes it.
pub fn read_frame<T: DeserializeOwned>(r: &mut impl Read) -> Result<T, FrameError> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len);
    if len > MAX_FRAME {
        return Err(FrameError::TooLarge(len));
    }
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload)?;
    decode(&payload)
}

/// Decodes a payload, requiring every byte to be used.
pub fn decode<T: DeserializeOwned>(payload: &[u8]) -> Result<T, FrameError> {
    match postcard::take_from_bytes(payload) {
        Ok((msg, [])) => Ok(msg),
        Ok(_) => Err(FrameError::Decode(postcard::Error::DeserializeBadEncoding)),
        Err(e) => Err(FrameError::Decode(e)),
    }
}

/// Property values that can cross the process boundary. Pointers never do.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Prop {
    Int(Vec<i32>),
    Double(Vec<f64>),
    String(Vec<String>),
}

/// A property set's sendable properties, sorted by name.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PropList(pub Vec<(String, Prop)>);

impl PropList {
    pub fn from_set(set: &PropSet) -> PropList {
        PropList(
            set.names()
                .into_iter()
                .filter_map(|n| {
                    let v = match set.get(&n)? {
                        Values::Int(v) => Prop::Int(v),
                        Values::Double(v) => Prop::Double(v),
                        Values::String(v) => Prop::String(
                            v.iter().map(|s| s.to_string_lossy().into_owned()).collect(),
                        ),
                        Values::Pointer(_) => return None,
                    };
                    Some((n, v))
                })
                .collect(),
        )
    }

    pub fn get(&self, name: &str) -> Option<&Prop> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    /// The first string of `name`, if it is a string property.
    pub fn string(&self, name: &str) -> Option<&str> {
        match self.get(name)? {
            Prop::String(v) => v.first().map(String::as_str),
            _ => None,
        }
    }
}

/// One parameter as Describe-in-context defined it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamDesc {
    pub name: String,
    pub kind: ParamKind,
    pub props: PropList,
}

/// One clip as Describe-in-context defined it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipDesc {
    pub name: String,
    pub props: PropList,
}

/// One supported context.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextDesc {
    pub context: String,
    pub props: PropList,
    pub clips: Vec<ClipDesc>,
    pub params: Vec<ParamDesc>,
}

/// One plugin of a bundle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginDesc {
    pub identifier: String,
    pub version: (u32, u32),
    /// The Describe properties.
    pub props: PropList,
    pub contexts: Vec<ContextDesc>,
}

/// Everything a bundle describes, plus the plugins that failed and why.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BundleDesc {
    pub plugins: Vec<PluginDesc>,
    /// `(identifier or index, reason)`.
    pub failed: Vec<(String, String)>,
}

/// A parameter's value at the request's time, and its key times, so the
/// plugin's key queries stay local (design §6.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamSnapshot {
    pub name: String,
    pub value: ParamValue,
    pub keys: Vec<f64>,
}

/// An image in shared memory.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ImageSlot {
    /// Byte offset of the bottom-left pixel in the mapping.
    pub offset: u64,
    pub bounds: [i32; 4],
    pub row_bytes: u32,
    pub depth: Depth,
    pub components: Components,
    pub premult: Premult,
}

/// A change the plugin made to a parameter (`paramSetValue` and friends),
/// which the editor turns into a command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ParamEdit {
    Set {
        name: String,
        time: Option<f64>,
        value: ParamValue,
    },
    DeleteKey {
        name: String,
        time: f64,
    },
    DeleteAllKeys {
        name: String,
    },
}

/// What the host asks the sandbox to do. Instances are numbered by the host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Request {
    Hello {
        version: u32,
    },
    /// Load and describe every plugin in the bundle.
    Describe,
    CreateInstance {
        instance: u32,
        plugin: String,
        context: String,
        project: Project,
        params: Vec<ParamSnapshot>,
    },
    DestroyInstance {
        instance: u32,
    },
    /// New parameter values, at `time`.
    SetParams {
        instance: u32,
        time: f64,
        params: Vec<ParamSnapshot>,
    },
    ClipPreferences {
        instance: u32,
    },
    RegionOfDefinition {
        instance: u32,
        time: f64,
        scale: [f64; 2],
    },
    RegionsOfInterest {
        instance: u32,
        time: f64,
        scale: [f64; 2],
        region: [f64; 4],
    },
    IsIdentity {
        instance: u32,
        time: f64,
        window: [i32; 4],
        scale: [f64; 2],
    },
    Render {
        instance: u32,
        time: f64,
        window: [i32; 4],
        scale: [f64; 2],
        inputs: Vec<(String, ImageSlot)>,
        output: ImageSlot,
    },
    Sequence {
        instance: u32,
        begin: bool,
        frames: [f64; 2],
        interactive: bool,
    },
    InstanceChanged {
        instance: u32,
        param: String,
        time: f64,
    },
    /// Unload every plugin and exit.
    Unload,
}

/// The sandbox's answer to one request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Reply {
    Hello {
        version: u32,
        pid: u32,
    },
    Described(BundleDesc),
    /// The action ran. Carries what the plugin said and changed meanwhile.
    Done {
        messages: Vec<Message>,
        edits: Vec<ParamEdit>,
    },
    ClipPreferences {
        clips: Vec<(String, Depth, Components)>,
        output_premult: Premult,
        frame_varying: bool,
    },
    Region([f64; 4]),
    Regions(Vec<(String, [f64; 4])>),
    /// `Some((clip, time))` when the effect is an identity.
    Identity(Option<(String, f64)>),
    /// The action returned an error status.
    Failed {
        action: String,
        status: i32,
        messages: Vec<Message>,
    },
    /// The request could not be carried out (unknown instance, bad slot).
    Error(String),
}

impl PluginDesc {
    /// The wire form of what Describe and Describe-in-context reported.
    pub fn from_description(
        identifier: &str,
        version: (u32, u32),
        d: &crate::suites::driver::Description,
    ) -> PluginDesc {
        PluginDesc {
            identifier: identifier.to_owned(),
            version,
            props: PropList::from_set(&d.general.props),
            contexts: d
                .contexts
                .iter()
                .map(|(name, fx)| ContextDesc {
                    context: name.clone(),
                    props: PropList::from_set(&fx.props),
                    clips: fx
                        .clips()
                        .iter()
                        .map(|c| ClipDesc {
                            name: c.name.clone(),
                            props: PropList::from_set(&c.props),
                        })
                        .collect(),
                    params: fx
                        .params
                        .params()
                        .iter()
                        .map(|p| ParamDesc {
                            name: p.name.clone(),
                            kind: p.kind,
                            props: PropList::from_set(&p.props),
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}
