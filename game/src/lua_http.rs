use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue},
    Method,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, str::FromStr};

use mlua::{self, Function, Lua, LuaSerdeExt, RegistryKey, UserData, UserDataMethods};

#[derive(Default)]
pub struct LuaHttp {
    calls: Vec<poll_promise::Promise<Response>>,
    callbacks: HashMap<i64, RegistryKey>,
    next_id: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Response {
    #[serde(skip)]
    id: i64,
    url: String,
    text: String,
    status: i32,
    elapsed: f32,
    error: String,
    cookies: String,
    header: HashMap<String, String>,
}

impl Response {
    /// A failed request. The `id` matters: `LuaHttp::poll` dispatches the
    /// callback by id, so a failure that forgot it would leave the Lua caller
    /// waiting forever (this is what made "Get Songs" hang on LOADING).
    pub fn error(id: i64, error: String) -> Self {
        Self {
            id,
            url: String::new(),
            text: String::new(),
            status: -1,
            elapsed: 0.0,
            error,
            cookies: String::new(),
            header: HashMap::new(),
        }
    }

    pub async fn from_response(response: reqwest::Response) -> Self {
        Self {
            id: -1,
            url: response.url().to_string(),
            status: response.status().as_u16() as _,
            elapsed: 1.0,
            error: String::new(),
            cookies: String::new(),
            header: response
                .headers()
                .iter()
                .map(|(name, value)| {
                    (
                        name.as_str().to_string(),
                        value.to_str().unwrap_or_default().to_string(),
                    )
                })
                .collect(),
            text: response.text().await.unwrap_or_default(),
        }
    }

    pub fn from_response_blocking(response: reqwest::blocking::Response) -> Self {
        Self {
            id: -1,
            url: response.url().to_string(),
            status: response.status().as_u16() as _,
            elapsed: 1.0,
            error: String::new(),
            cookies: String::new(),
            header: response
                .headers()
                .iter()
                .map(|(name, value)| {
                    (
                        name.as_str().to_string(),
                        value.to_str().unwrap_or_default().to_string(),
                    )
                })
                .collect(),
            text: response.text().unwrap_or_default(),
        }
    }
}

impl LuaHttp {
    pub fn poll(lua: &Lua) {
        let (mut calls, mut callbacks) = {
            let mut http = lua
                .app_data_mut::<LuaHttp>()
                .expect("LuaHttp app data not set");

            (
                std::mem::take(&mut http.calls),
                std::mem::take(&mut http.callbacks),
            )
        };

        let mut remaining_calls = vec![];
        // A request that never resolves keeps the Lua caller on its loading
        // screen; report it periodically so a hang can be told apart from a
        // slow network.
        if !calls.is_empty() {
            static PENDING_POLLS: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            if PENDING_POLLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 300 == 0 {
                log::info!("LuaHttp::poll: {} request(s) still pending", calls.len());
            }
        }
        for ele in calls.drain(..) {
            match ele.try_take() {
                Ok(data) => {
                    log::info!(
                        "LuaHttp::poll resolved id={} status={}",
                        data.id,
                        data.status
                    );
                    if let Some(key) = callbacks.remove(&data.id) {
                        if let Ok(callback) = lua.registry_value::<Function>(&key) {
                            if let Ok(value) = lua.to_value(&data) {
                                log::info!("LuaHttp::poll invoking callback id={}", data.id);
                                if let Err(e) = callback.call::<()>(value) {
                                    log::error!("Http callback id={} failed: {e}", data.id);
                                }
                                log::info!("LuaHttp::poll callback id={} returned", data.id);
                            }
                        }
                    } else {
                        log::warn!("LuaHttp::poll: no callback registered for id={}", data.id);
                    }
                }
                Err(call) => remaining_calls.push(call),
            }
        }

        {
            let mut http = lua
                .app_data_mut::<LuaHttp>()
                .expect("LuaHttp app data not set");

            http.calls.append(&mut remaining_calls);
            for (id, key) in callbacks.drain() {
                http.callbacks.insert(id, key);
            }
        }
    }
}

#[derive(Default)]
pub struct ExportLuaHttp;

impl UserData for ExportLuaHttp {
    fn add_methods<T: UserDataMethods<Self>>(methods: &mut T) {
        methods.add_function(
            "Get",
            |lua, (url, headers): (String, HashMap<String, String>)| {
                let mut req = reqwest::blocking::Request::new(
                    Method::GET,
                    reqwest::Url::parse(&url).map_err(mlua::Error::external)?,
                );

                for (header, value) in headers.iter() {
                    req.headers_mut().append(
                        HeaderName::from_str(header).map_err(mlua::Error::external)?,
                        HeaderValue::from_str(value).map_err(mlua::Error::external)?,
                    );
                }
                lua.to_value(
                    &reqwest::blocking::Client::new()
                        .execute(req)
                        .map(Response::from_response_blocking)
                        .map_err(mlua::Error::external)?,
                )
            },
        );

        methods.add_function(
            "Post",
            |lua, (url, content, headers): (String, String, HashMap<String, String>)| {
                let client = reqwest::blocking::Client::builder()
                    .build()
                    .map_err(mlua::Error::external)?;

                let mut req = client.post(url).body(content);
                for (header, value) in headers.iter() {
                    req = req.header(header, value);
                }

                let req = req.build().map_err(mlua::Error::external)?;

                lua.to_value(
                    &client
                        .execute(req)
                        .map(Response::from_response_blocking)
                        .map_err(mlua::Error::external)?,
                )
            },
        );

        methods.add_function(
            "GetAsync",
            |lua, (url, headers, callback): (String, HashMap<String, String>, Function)| {
                if let Some(mut http) = lua.app_data_mut::<LuaHttp>() {
                    let id = http.next_id;
                    http.callbacks
                        .insert(id, lua.create_registry_value(callback)?);

                    http.calls
                        .push(poll_promise::Promise::spawn_async(async move {
                            log::info!("Http.GetAsync id={id} url={url}");
                            let client = match reqwest::Client::builder()
                                .default_headers(HeaderMap::from_iter(headers.iter().filter_map(
                                    |(name, value)| {
                                        // A malformed header must not take the
                                        // whole app down: `HeaderName::from_static`
                                        // aborts the process on invalid bytes, and
                                        // skins are free to pass anything. Drop the
                                        // entry and keep the request going.
                                        let name: HeaderName = match name.parse() {
                                            Ok(name) => name,
                                            Err(e) => {
                                                log::warn!("Skipping header {name:?}: {e}");
                                                return None;
                                            }
                                        };
                                        let value: HeaderValue = match value.parse() {
                                            Ok(value) => value,
                                            Err(e) => {
                                                log::warn!("Skipping header {name:?}: {e}");
                                                return None;
                                            }
                                        };
                                        Some((name, value))
                                    },
                                )))
                                .build()
                            {
                                Ok(v) => v,
                                Err(e) => {
                                    log::warn!("Http.GetAsync id={id} client build failed: {e}");
                                    return Response::error(id, format!("{e}"));
                                }
                            };

                            // A request that never completes would leave the
                            // Lua caller waiting forever, so bound it.
                            let req = match client
                                .get(url)
                                .timeout(std::time::Duration::from_secs(25))
                                .build()
                            {
                                Ok(v) => v,
                                Err(e) => {
                                    log::warn!("Http.GetAsync id={id} request build failed: {e}");
                                    return Response::error(id, format!("{e}"));
                                }
                            };

                            match client.execute(req).await.map(Response::from_response) {
                                Ok(r) => {
                                    let mut r = r.await;
                                    r.id = id;
                                    log::info!(
                                        "Http.GetAsync id={id} status={} bytes={}",
                                        r.status,
                                        r.text.len()
                                    );
                                    r
                                }
                                Err(e) => {
                                    log::warn!("Http.GetAsync id={id} failed: {e:?}");
                                    Response::error(id, format!("{:?}", e))
                                }
                            }
                        }));

                    http.next_id += 1;
                } else {
                    log::error!("Http.GetAsync: LuaHttp app data is not registered");
                }
                Ok(())
            },
        );

        methods.add_function(
            "PostAsync",
            |lua,
             (url, content, headers, callback): (
                String,
                String,
                HashMap<String, String>,
                Function,
            )| {
                if let Some(mut http) = lua.app_data_mut::<LuaHttp>() {
                    let id = http.next_id;
                    http.callbacks
                        .insert(id, lua.create_registry_value(callback)?);

                    http.calls
                        .push(poll_promise::Promise::spawn_async(async move {
                            let client = match reqwest::Client::builder()
                                .default_headers(HeaderMap::from_iter(headers.iter().filter_map(
                                    |(name, value)| {
                                        let name: HeaderName = match name.parse() {
                                            Ok(name) => name,
                                            Err(e) => {
                                                log::warn!("Skipping header {name:?}: {e}");
                                                return None;
                                            }
                                        };
                                        let value: HeaderValue = match value.parse() {
                                            Ok(value) => value,
                                            Err(e) => {
                                                log::warn!("Skipping header {name:?}: {e}");
                                                return None;
                                            }
                                        };
                                        Some((name, value))
                                    },
                                )))
                                .build()
                            {
                                Ok(v) => v,
                                Err(e) => {
                                    log::warn!("Http.PostAsync id={id} client build failed: {e}");
                                    return Response::error(id, e.to_string());
                                }
                            };

                            let request = match client
                                .post(url)
                                .body(content)
                                .timeout(std::time::Duration::from_secs(25))
                                .build()
                            {
                                Ok(v) => v,
                                Err(e) => {
                                    log::warn!("Http.PostAsync id={id} request build failed: {e}");
                                    return Response::error(id, e.to_string());
                                }
                            };

                            match client.execute(request).await.map(Response::from_response) {
                                Ok(r) => {
                                    let mut r = r.await;
                                    r.id = id;
                                    log::info!(
                                        "Http.PostAsync id={id} status={} bytes={}",
                                        r.status,
                                        r.text.len()
                                    );
                                    r
                                }
                                Err(e) => {
                                    log::warn!("Http.PostAsync id={id} failed: {e:?}");
                                    Response::error(id, format!("{:?}", e))
                                }
                            }
                        }));

                    http.next_id += 1;
                }
                Ok(())
            },
        )
    }
}
