use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub enum Request {
    Anim(Vec<(Vec<u8>, Vec<String>)>),
    Clear([u8; 3], Vec<String>),
    Img(Vec<(Vec<u8>, Vec<String>)>, u8),
    Init,
    Kill,
    Query,
}

#[derive(Serialize, Deserialize)]
pub enum Answer {
    Ok,
    Err(String),
    Query(Vec<(String, (u32, u32))>),
}
