use std::collections::HashMap;
use zellij_remote_protocol::Style;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StyleKey {
    bytes: Vec<u8>,
}

impl StyleKey {
    fn from_style(style: &Style) -> Self {
        use prost::Message;
        let mut bytes = Vec::new();
        style.encode(&mut bytes).unwrap();
        Self { bytes }
    }
}

pub struct StyleTable {
    styles: Vec<Style>,
    style_to_id: HashMap<StyleKey, u16>,
}

impl StyleTable {
    pub fn new() -> Self {
        let mut table = Self {
            styles: Vec::new(),
            style_to_id: HashMap::new(),
        };
        table.styles.push(Style::default());
        table
    }

    pub fn get_or_insert(&mut self, style: &Style) -> u16 {
        let key = StyleKey::from_style(style);

        if let Some(&id) = self.style_to_id.get(&key) {
            return id;
        }

        let id = self.styles.len() as u16;
        self.styles.push(style.clone());
        self.style_to_id.insert(key, id);
        id
    }

    pub fn get(&self, id: u16) -> Option<&Style> {
        self.styles.get(id as usize)
    }

    pub fn current_count(&self) -> usize {
        self.styles.len()
    }

    pub fn styles_since(&self, baseline: usize) -> Vec<(u16, &Style)> {
        self.styles
            .iter()
            .enumerate()
            .skip(baseline)
            .map(|(id, style)| (id as u16, style))
            .collect()
    }

    pub fn reset(&mut self) {
        self.styles.truncate(1);
        self.style_to_id.clear();
    }

    pub fn all_styles(&self) -> impl Iterator<Item = (u16, &Style)> {
        self.styles
            .iter()
            .enumerate()
            .map(|(id, style)| (id as u16, style))
    }
}

impl Default for StyleTable {
    fn default() -> Self {
        Self::new()
    }
}
