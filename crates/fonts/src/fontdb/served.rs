use super::names::{base_style, same_style, style_italic, style_weight};
use super::{FontDb, ServedFace};

impl FontDb {
    pub fn register_served_faces(&self, faces: Vec<ServedFace>) {
        let mut served = self.served.write().unwrap_or_else(|e| e.into_inner());
        for face in faces {
            if !face.family.trim().is_empty()
                && !face.style.trim().is_empty()
                && !served.iter().any(|f| f.family.eq_ignore_ascii_case(&face.family) && same_style(&f.style, &face.style))
            {
                served.push(face);
            }
        }
    }

    pub fn take_requests(&self) -> Vec<(String, String)> {
        std::mem::take(&mut *self.requests.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn is_loaded(&self, family: &str, style: &str) -> bool {
        self.scoped(0).is_loaded(family, style)
    }

    pub(super) fn request_served(&self, family: &str, style: &str) {
        let request = {
            let served = self.served.read().unwrap_or_else(|e| e.into_inner());
            let matches = || served.iter().filter(|f| f.family.eq_ignore_ascii_case(family));
            let style = base_style(style);
            matches()
                .find(|f| same_style(&f.style, style))
                .or_else(|| {
                    let distance = |f: &ServedFace| {
                        (f32::from(f.weight) - style_weight(style)).abs() + if f.italic != style_italic(style) { 1000.0 } else { 0.0 }
                    };
                    matches().min_by(|a, b| distance(a).total_cmp(&distance(b)))
                })
                .map(|f| (f.family.clone(), f.style.clone()))
        };
        if let Some(request) = request
            && !self.is_loaded(&request.0, &request.1)
        {
            let mut requests = self.requests.lock().unwrap_or_else(|e| e.into_inner());
            if !requests.contains(&request) {
                requests.push(request);
            }
        }
    }
}
