// SPDX-License-Identifier: AGPL-3.0-or-later

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use zhiye_sync_server::{Engine, Service, ServiceResult, serve};

const USAGE: &str = "用法：zhiye-sync-server --data <目录> [--listen 127.0.0.1:8787] [--behind-tls-proxy]
                          [--admin-token-file <文件>]
                          [--database-url <连接参数>] [--database-password-file <文件>]

--data            存放附件原件；未指定数据库时还存放 sync.sqlite。
--database-url    使用 PostgreSQL，可写 URL 或 key=value 形式，也可用环境变量 ZHIYE_SYNC_DATABASE_URL。
                  密码建议放在 --database-password-file 或 ZHIYE_SYNC_DATABASE_PASSWORD_FILE 指定的文件中。
管理令牌从 --admin-token-file、ZHIYE_SYNC_ADMIN_TOKEN_FILE 或 ZHIYE_SYNC_ADMIN_TOKEN 读取，至少 32 个字符。
默认只监听本机回环地址。监听其他地址时必须由 HTTPS 反向代理转发，并加 --behind-tls-proxy。";

/// How long to keep retrying a database that is still starting.
const DATABASE_WAIT: Duration = Duration::from_secs(60);

struct Options {
    data: PathBuf,
    listen: SocketAddr,
    admin_token: String,
    database: Option<String>,
    database_password: Option<String>,
}

fn read_secret(path: &PathBuf, label: &str) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map(|text| text.trim().to_owned())
        .map_err(|error| format!("无法读取{label}文件 {}：{error}", path.display()))
}

fn options() -> Result<Options, String> {
    let mut data = None;
    let mut listen: SocketAddr = "127.0.0.1:8787".parse().expect("default address");
    let mut token_file = std::env::var_os("ZHIYE_SYNC_ADMIN_TOKEN_FILE").map(PathBuf::from);
    let mut database = std::env::var("ZHIYE_SYNC_DATABASE_URL").ok();
    let mut password_file =
        std::env::var_os("ZHIYE_SYNC_DATABASE_PASSWORD_FILE").map(PathBuf::from);
    let mut behind_proxy = false;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--data" => data = arguments.next().map(PathBuf::from),
            "--listen" => {
                listen = arguments
                    .next()
                    .and_then(|value| value.parse().ok())
                    .ok_or("--listen 需要 IP:端口 格式的地址。")?
            }
            "--admin-token-file" => token_file = arguments.next().map(PathBuf::from),
            "--database-url" => database = arguments.next(),
            "--database-password-file" => password_file = arguments.next().map(PathBuf::from),
            "--behind-tls-proxy" => behind_proxy = true,
            "--help" | "-h" => return Err(USAGE.into()),
            other => return Err(format!("未知参数 {other}。\n\n{USAGE}")),
        }
    }
    let data = data.ok_or_else(|| format!("缺少 --data。\n\n{USAGE}"))?;
    if !listen.ip().is_loopback() && !behind_proxy {
        return Err(
            "监听非回环地址时，需要由 HTTPS 反向代理提供传输加密，并显式加 --behind-tls-proxy。"
                .into(),
        );
    }
    let admin_token = match token_file {
        Some(path) => read_secret(&path, "管理令牌")?,
        None => std::env::var("ZHIYE_SYNC_ADMIN_TOKEN").map_err(|_| {
            "需要提供管理令牌：--admin-token-file、ZHIYE_SYNC_ADMIN_TOKEN_FILE 或 ZHIYE_SYNC_ADMIN_TOKEN。"
        })?,
    };
    let database_password = password_file
        .map(|path| read_secret(&path, "数据库密码"))
        .transpose()?;
    if database_password.is_some() && database.is_none() {
        return Err("提供了数据库密码，但没有指定 --database-url。".into());
    }
    Ok(Options {
        data,
        listen,
        admin_token,
        database: database.filter(|value| !value.trim().is_empty()),
        database_password,
    })
}

/// Opens the store before the async runtime starts; the PostgreSQL client
/// blocks on its own runtime. A database that is still starting is retried.
fn open(options: &Options) -> ServiceResult<Engine> {
    let Some(database) = &options.database else {
        return Engine::open(&options.data);
    };
    let blobs = options.data.join("blobs");
    let started = std::time::Instant::now();
    loop {
        match Engine::open_postgres(database, options.database_password.as_deref(), &blobs) {
            Err(error) if error.code == "STORAGE" && started.elapsed() < DATABASE_WAIT => {
                eprintln!("等待 PostgreSQL 就绪：{error}");
                std::thread::sleep(Duration::from_secs(2));
            }
            result => return result,
        }
    }
}

fn main() -> ExitCode {
    let options = match options() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let service = match open(&options).and_then(|engine| Service::new(engine, &options.admin_token))
    {
        Ok(service) => Arc::new(service),
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let storage = if options.database.is_some() {
        "PostgreSQL"
    } else {
        "SQLite"
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("无法启动服务线程：{error}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(async move {
        let listener = match tokio::net::TcpListener::bind(options.listen).await {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("无法监听 {}：{error}", options.listen);
                return ExitCode::FAILURE;
            }
        };
        let address = listener.local_addr().unwrap_or(options.listen);
        println!(
            "zhiye-sync-server {} 已在 http://{address} 监听，存储：{storage}",
            env!("CARGO_PKG_VERSION")
        );
        serve(listener, service).await;
        ExitCode::FAILURE
    })
}
