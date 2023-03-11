use crate::{error::*, notification::Notification, xdg};
use std::path::PathBuf;
use zbus::{export::futures_util::TryStreamExt, MatchRule};

use super::{
    ActionResponse, ActionResponseHandler, CloseReason, NOTIFICATION_NAMESPACE,
    NOTIFICATION_OBJECTPATH,
};

/// A handle to a shown notification.
///
/// This keeps a connection alive to ensure actions work on certain desktops.
#[derive(Debug)]
pub struct ZbusNotificationHandle {
    pub(crate) id: u32,
    pub(crate) connection: zbus::Connection,
    pub(crate) notification: Notification,
}

impl ZbusNotificationHandle {
    pub(crate) fn new(
        id: u32,
        connection: zbus::Connection,
        notification: Notification,
    ) -> ZbusNotificationHandle {
        ZbusNotificationHandle {
            id,
            connection,
            notification,
        }
    }

    pub async fn wait_for_action(self, invocation_closure: impl ActionResponseHandler) {
        log::trace!("wait_for_action...");
        wait_for_action_signal(&self.connection, self.id, invocation_closure).await;
        log::trace!("wait_for_action. done");
    }

    pub async fn close_fallible(self) -> Result<()> {
        log::trace!("close id {}", self.id);
        self.connection
            .call_method(
                Some(NOTIFICATION_NAMESPACE),
                NOTIFICATION_OBJECTPATH,
                Some(NOTIFICATION_NAMESPACE),
                "CloseNotification",
                &(self.id),
            )
            .await?;
        Ok(())
    }

    pub async fn close(self) {
        self.close_fallible().await.unwrap();
    }

    pub fn on_close<F>(self, closure: F)
    where
        F: FnOnce(CloseReason),
    {
        zbus::block_on(self.wait_for_action(|action: &ActionResponse| {
            if let ActionResponse::Closed(reason) = action {
                closure(*reason);
            }
        }));
    }

    pub fn update_fallible(&mut self) -> Result<()> {
        self.id = zbus::block_on(send_notification_via_connection(
            &self.notification,
            self.id,
            &self.connection,
        ))?;
        Ok(())
    }

    pub fn update(&mut self) {
        self.update_fallible().unwrap();
    }
}

#[derive(Debug)]
pub(crate) struct NotificationObjectPath(String);

impl<I> From<I> for NotificationObjectPath
where
    // I: std::fmt::Display,
    I: ToString,
{
    fn from(value: I) -> Self {
        NotificationObjectPath(value.to_string())
    }
}

impl NotificationObjectPath {
    // #[cfg(test)]
    // pub(crate) fn new(custom_path: impl ToString) -> Self {
    //     Self::from(custom_path)
    // }

    pub(crate) fn custom(custom_path: &str) -> Option<Self> {
        let namespaced_custom = PathBuf::from("/de/hoodie/Notification")
            .join(custom_path)
            .to_str()?
            .to_owned();

        Some(Self::from(namespaced_custom))
    }
}

impl Default for NotificationObjectPath {
    fn default() -> Self {
        Self(String::from(NOTIFICATION_OBJECTPATH))
    }
}

impl TryFrom<&NotificationObjectPath> for zbus::zvariant::ObjectPath<'static> {
    type Error = zbus::zvariant::Error;

    fn try_from(value: &NotificationObjectPath) -> std::result::Result<Self, Self::Error> {
        zbus::zvariant::ObjectPath::try_from(value.0.to_owned())
    }
}

async fn send_notification_via_connection(
    notification: &Notification,
    id: u32,
    connection: &zbus::Connection,
) -> Result<u32> {
    send_notification_via_connection_at_path(notification, id, connection, Default::default()).await
}

async fn send_notification_via_connection_at_path(
    notification: &Notification,
    id: u32,
    connection: &zbus::Connection,
    path: NotificationObjectPath,
) -> Result<u32> {
    log::trace!(
        "send_notification_via_connection ({NOTIFICATION_NAMESPACE}) at {:?}",
        path
    );
    // if let Some(ref close_handler) = notification.close_handler {
    //     // close_handler.
    //     let connection = connection.clone();
    //     async_std::task::spawn(async move {
    //         wait_for_action_signal(&connection, id, |response: &ActionResponse<'_>| log::trace!("{:?}", response))
    //     });
    // }
    let reply: u32 = connection
        .call_method(
            Some(NOTIFICATION_NAMESPACE),
            &path,
            Some(NOTIFICATION_NAMESPACE),
            "Notify",
            &(
                &notification.appname,
                id,
                &notification.icon,
                &notification.summary,
                &notification.body,
                &notification.actions,
                crate::hints::hints_to_map(notification),
                i32::from(notification.timeout),
            ),
        )
        .await?
        .body()
        .unwrap();
    Ok(reply)
}

pub async fn connect_and_send_notification(
    notification: &Notification,
) -> Result<ZbusNotificationHandle> {
    connect_and_send_notification_at_path(notification, Default::default()).await
}

pub(crate) async fn connect_and_send_notification_at_path(
    notification: &Notification,
    path: NotificationObjectPath,
) -> Result<ZbusNotificationHandle> {
    let connection = zbus::Connection::session().await?;
    let inner_id = notification.id.unwrap_or(0);
    let id =
        send_notification_via_connection_at_path(notification, inner_id, &connection, path).await?;
    Ok(ZbusNotificationHandle::new(
        id,
        connection,
        notification.clone(),
    ))
}

pub async fn get_capabilities() -> Result<Vec<String>> {
    log::trace!("get_capabilities()");
    let connection = zbus::Connection::session().await?;
    let info: Vec<String> = connection
        .call_method(
            Some(NOTIFICATION_NAMESPACE),
            NOTIFICATION_OBJECTPATH,
            Some(NOTIFICATION_NAMESPACE),
            "GetCapabilities",
            &(),
        )
        .await?
        .body()?;

    Ok(info)
}

pub async fn get_server_information() -> Result<xdg::ServerInformation> {
    log::trace!("get_server_information()");
    let connection = zbus::Connection::session().await?;
    let info: xdg::ServerInformation = connection
        .call_method(
            Some(NOTIFICATION_NAMESPACE),
            NOTIFICATION_OBJECTPATH,
            Some(NOTIFICATION_NAMESPACE),
            "GetServerInformation",
            &(),
        )
        .await?
        .body()?;

    Ok(info)
}

/// Listens for the `ActionInvoked(UInt32, String)` Signal.
///
/// No need to use this, check out `Notification::show_and_wait_for_action(FnOnce(action:&str))`
pub async fn handle_action(id: u32, func: impl ActionResponseHandler) {
    log::trace!("handle_action");
    let connection = zbus::Connection::session().await.unwrap();
    wait_for_action_signal(&connection, id, func).await;
}

async fn wait_for_action_signal(
    connection: &zbus::Connection,
    id: u32,
    handler: impl ActionResponseHandler,
) {
    let action_signal_rule = MatchRule::builder()
        .msg_type(zbus::MessageType::Signal)
        .interface(NOTIFICATION_NAMESPACE)
        .unwrap()
        .member("ActionInvoked")
        .unwrap()
        .build();

    let proxy = zbus::fdo::DBusProxy::new(connection).await.unwrap();
    proxy.add_match_rule(action_signal_rule).await.unwrap();

    let close_signal_rule = MatchRule::builder()
        .msg_type(zbus::MessageType::Signal)
        .interface(NOTIFICATION_NAMESPACE)
        .unwrap()
        .member("NotificationClosed")
        .unwrap()
        .build();
    proxy.add_match_rule(close_signal_rule).await.unwrap();

    while let Ok(Some(msg)) = zbus::MessageStream::from(connection).try_next().await {
        if let Ok(header) = msg.header() {
            log::trace!("signal received {:?}", header);

            if let Ok(zbus::MessageType::Signal) = header.message_type() {
                log::trace!("it's a signal message");

                match header.member() {
                    Ok(Some(name)) if name == "ActionInvoked" => {
                        match msg.body::<(u32, String)>() {
                            Ok((nid, action)) if nid == id => {
                                log::trace!("ActionInvoked {}", action);
                                handler.call(&ActionResponse::Custom(&action));
                                break;
                            }
                            other => {
                                log::warn!("ActionInvoked failed {:?}", other);
                            }
                        }
                    }
                    Ok(Some(name)) if name == "NotificationClosed" => {
                        match msg.body::<(u32, u32)>() {
                            Ok((nid, reason)) if nid == id => {
                                let reason: CloseReason = reason.into();
                                log::trace!("Notification Closed {:?}", reason);
                                handler.call(&ActionResponse::Closed(reason));
                                break;
                            }
                            other => {
                                log::warn!("NotificationClosed failed {:?}", other);
                            }
                        }
                    }
                    Ok(_) => {
                        log::trace!("received unhandled signal");
                    }
                    Err(error) => {
                        log::trace!("failed to handle message {}", error);
                    }
                }
            }
        } else {
            log::warn!("received unexpected message");
        }
    }
}

pub fn stop_server() -> Result<()> {
    let connection = zbus::blocking::Connection::session()?;
    connection.call_method(
        Some(NOTIFICATION_NAMESPACE),
        NOTIFICATION_OBJECTPATH,
        Some(NOTIFICATION_NAMESPACE),
        "Stop",
        &(),
    )?;

    Ok(())
}
