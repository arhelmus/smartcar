//! Linux GATT peripheral implementation via BlueZ (`bluer`).
//!
//! Builds an `Application` with one service and three characteristics
//! (Command, Event, Info) and starts advertising. Keeps the application and
//! advertisement handles alive for the lifetime of [`run`].

use std::sync::Arc;

use bluer::{
    adv::Advertisement,
    gatt::local::{
        Application, Characteristic, CharacteristicNotify, CharacteristicNotifyMethod,
        CharacteristicRead, CharacteristicWrite, CharacteristicWriteMethod, Service,
    },
};
use futures::FutureExt;
use prost::Message;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};

use crate::{
    proto::{ControlEvent, ControlRequest},
    DeviceInfo, COMMAND_UUID, EVENT_UUID, INFO_UUID, SERVICE_UUID,
};

/// Power the default Bluetooth adapter, register the smartcar bridge GATT
/// application, start advertising, and park the task.
///
/// `cmd_tx` receives decoded [`ControlRequest`]s written by the phone.
/// `evt_tx` is the broadcast source for [`ControlEvent`]s the bridge
/// notifies to subscribed centrals; the bridge calls `subscribe()` once
/// per active notify session.
///
/// The future never returns under normal operation. Drop the surrounding
/// task to stop advertising and tear down the GATT application.
pub(crate) async fn run(
    device_info: DeviceInfo,
    cmd_tx: mpsc::Sender<ControlRequest>,
    evt_tx: broadcast::Sender<ControlEvent>,
) -> anyhow::Result<()> {
    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;
    // Resolve the adapter address before the `info!` call: awaiting inside the
    // macro arguments leaves a non-Send tracing value held across the await,
    // which makes `gatt::run`'s Future !Send and breaks `tokio::spawn`.
    let address = adapter.address().await?;
    info!(%address, "bridge: bluetooth adapter ready");

    // Info is static — encode once and clone the Arc on every read.
    let info_bytes: Arc<Vec<u8>> = {
        let proto = device_info.to_proto();
        let mut buf = Vec::with_capacity(proto.encoded_len());
        proto.encode(&mut buf).expect("encode Info");
        Arc::new(buf)
    };

    let app = Application {
        services: vec![Service {
            uuid: SERVICE_UUID,
            primary: true,
            characteristics: vec![
                command_char(cmd_tx),
                event_char(evt_tx),
                info_char(info_bytes),
            ],
            ..Default::default()
        }],
        ..Default::default()
    };
    let _app_handle = adapter.serve_gatt_application(app).await?;

    let adv = Advertisement {
        service_uuids: [SERVICE_UUID].into_iter().collect(),
        discoverable: Some(true),
        local_name: Some(device_info.name.clone()),
        ..Default::default()
    };
    let _adv_handle = adapter.advertise(adv).await?;
    info!(
        service = %SERVICE_UUID,
        name = %device_info.name,
        "bridge: advertising"
    );

    // Keep the handles alive; dropping them would stop advertising and
    // unregister the GATT app.
    std::future::pending::<()>().await;
    Ok(())
}

fn command_char(cmd_tx: mpsc::Sender<ControlRequest>) -> Characteristic {
    Characteristic {
        uuid: COMMAND_UUID,
        write: Some(CharacteristicWrite {
            write: true,
            write_without_response: true,
            method: CharacteristicWriteMethod::Fun(Box::new(move |value, _req| {
                let tx = cmd_tx.clone();
                async move {
                    match ControlRequest::decode(value.as_slice()) {
                        Ok(req) => {
                            debug!(?req, "bridge: command");
                            if tx.send(req).await.is_err() {
                                warn!("bridge: command receiver dropped");
                            }
                        }
                        Err(e) => warn!(error = %e, "bridge: invalid ControlRequest"),
                    }
                    Ok(())
                }
                .boxed()
            })),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn event_char(evt_tx: broadcast::Sender<ControlEvent>) -> Characteristic {
    Characteristic {
        uuid: EVENT_UUID,
        notify: Some(CharacteristicNotify {
            notify: true,
            method: CharacteristicNotifyMethod::Fun(Box::new(move |mut notifier| {
                let mut rx = evt_tx.subscribe();
                async move {
                    info!("bridge: central subscribed to events");
                    loop {
                        match rx.recv().await {
                            Ok(evt) => {
                                let mut buf = Vec::with_capacity(evt.encoded_len());
                                if let Err(e) = evt.encode(&mut buf) {
                                    warn!(error = %e, "bridge: encode event failed");
                                    continue;
                                }
                                if let Err(e) = notifier.notify(buf).await {
                                    info!(error = %e, "bridge: notify failed — central gone");
                                    break;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                warn!(lagged = n, "bridge: event broadcast lagged");
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                info!("bridge: event broadcast closed");
                                break;
                            }
                        }
                    }
                }
                .boxed()
            })),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn info_char(info_bytes: Arc<Vec<u8>>) -> Characteristic {
    Characteristic {
        uuid: INFO_UUID,
        read: Some(CharacteristicRead {
            read: true,
            fun: Box::new(move |_req| {
                let bytes = info_bytes.clone();
                async move { Ok((*bytes).clone()) }.boxed()
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}
