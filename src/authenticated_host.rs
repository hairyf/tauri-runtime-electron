#[derive(Debug, Clone)]
pub struct AuthenticatedWebHost(electron_rs::rpc::protocol::AuthenticatedHost);

impl AuthenticatedWebHost {
  pub(crate) fn id(&self) -> &str { &self.0.id }
  pub fn document_url(&self) -> &str { &self.0.document_url }
  pub fn backend_origin(&self) -> &str { &self.0.backend_origin }
}

pub async fn authenticate_web_host(
  client: &crate::ElectronClient, launch_url: &str,
) -> Result<AuthenticatedWebHost, electron_rs::Error> {
  client.call(electron_rs::rpc::protocol::AuthenticateHost { url: launch_url.to_owned() })
    .await.map(AuthenticatedWebHost)
}

#[cfg(feature = "tauri")]
pub fn bind_authenticated_web_host(
  webview: &tauri::Webview<crate::ElectronRuntime>, host: &AuthenticatedWebHost,
) -> tauri::Result<()> {
  let host_id = host.0.id.clone();
  crate::browser::route(webview, move |view| {
    view.webview_call(electron_rs::rpc::protocol::BindHost { webview_id: view.id, host_id })
  })
}
