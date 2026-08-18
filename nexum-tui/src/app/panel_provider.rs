use super::*;

impl App {
    /// Open the read-only `/proveedor` catalog panel (Global scope).
    ///
    /// Carga el catálogo que resuelve `catalog_path` once. If the catalog is missing or
    /// invalid, the panel renders an explicit error message instead of any data.
    pub fn open_provider_panel(&mut self) {
        let panel = provider_panel::ProviderPanel::load();
        self.open_panel(PanelState::Provider(panel));
    }

    /// Close the `/proveedor` catalog panel.
    pub fn close_provider_panel(&mut self) {
        self.global_panels.close_if(PanelKind::Provider);
    }
}
