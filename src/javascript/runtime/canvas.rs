//! Partial synchronous context bindings.
//!
//! <https://html.spec.whatwg.org/multipage/canvas.html#canvasrenderingcontext2d>

use super::{ElementMethod, Interpreter, JsError, ObjectStorage, Value};
use crate::{
    css::parse_color,
    graphics::{
        canvas::{Canvas2D, MAX_PAGE_CANVAS_PIXELS, MAX_PAGE_CANVASES, parse_canvas_dimension},
        raster::Color,
    },
};

impl Interpreter {
    pub(in crate::javascript) fn take_canvas_surfaces(
        &mut self,
    ) -> std::collections::HashMap<u64, Canvas2D> {
        let ids: Vec<_> = self.canvas_surfaces.keys().copied().collect();

        for node_id in ids {
            self.ensure_canvas_surface(node_id);
        }

        std::mem::take(&mut self.canvas_surfaces)
    }

    pub(super) fn set_canvas_property(
        &self,
        object: &ObjectStorage,
        name: &str,
        value: &Value,
    ) -> bool {
        if name != "fillStyle"
            || !self
                .canvas_contexts
                .values()
                .any(|context| std::rc::Rc::ptr_eq(context, object))
        {
            return false;
        }

        if let Value::String(value) = value {
            let value = value.trim();

            let normalized = if value.eq_ignore_ascii_case("transparent") {
                Some("transparent".to_owned())
            } else {
                parse_color(value).map(|rgb| format!("#{rgb:06x}"))
            };

            if let Some(normalized) = normalized {
                object
                    .borrow_mut()
                    .set("fillStyle".to_owned(), Value::String(normalized));
            }
        }

        true
    }

    pub(super) fn get_canvas_context(
        &mut self,
        node_id: u64,
        receiver: Option<Value>,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        if !matches!(receiver, Some(Value::Element(id)) if id == node_id) {
            return Err(JsError::type_error(
                "HTMLCanvasElement.getContext: invalid receiver",
            ));
        }

        let Some(kind) = arguments.first() else {
            return Err(JsError::type_error(
                "HTMLCanvasElement.getContext requires an argument",
            ));
        };

        if kind.to_js_string() != "2d" {
            return Ok(Value::Null);
        }

        if let Some(context) = self.canvas_contexts.get(&node_id) {
            return Ok(Value::Object(context.clone()));
        }

        if !self.ensure_canvas_surface(node_id) {
            return Ok(Value::Null);
        }

        let Value::Object(context) = Value::object_with_entries_and_prototype(
            [
                ("canvas".to_owned(), Value::Element(node_id)),
                ("fillStyle".to_owned(), Value::String("#000000".to_owned())),
            ],
            self.object_prototype.clone(),
        ) else {
            unreachable!("object constructor returns an object");
        };

        context.borrow_mut().define_own_data_property(
            "fillRect".to_owned(),
            Value::ElementMethod(ElementMethod::CanvasFillRect(node_id)),
            false,
        );

        self.canvas_contexts.insert(node_id, context.clone());

        Ok(Value::Object(context))
    }

    pub(super) fn ensure_canvas_surface(&mut self, node_id: u64) -> bool {
        let Some((width, height)) = self.current_document_element(node_id).map(|element| {
            let attribute = |name: &str| {
                element
                    .attributes
                    .iter()
                    .find(|attribute| attribute.name == name)
                    .and_then(|attribute| parse_canvas_dimension(&attribute.value))
            };

            (
                attribute("width").unwrap_or(300),
                attribute("height").unwrap_or(150),
            )
        }) else {
            return false;
        };

        if let Some(existing) = self.canvas_surfaces.get(&node_id) {
            if existing.surface().width() == width && existing.surface().height() == height {
                return true;
            }
        }

        self.canvas_surfaces.remove(&node_id);

        let used: usize = self
            .canvas_surfaces
            .values()
            .map(|canvas| canvas.surface().width() * canvas.surface().height())
            .sum();

        let Some(total) = width
            .checked_mul(height)
            .and_then(|pixels| used.checked_add(pixels))
        else {
            return false;
        };

        if total > MAX_PAGE_CANVAS_PIXELS || self.canvas_surfaces.len() >= MAX_PAGE_CANVASES {
            return false;
        }

        let Some(canvas) = Canvas2D::new(width, height) else {
            return false;
        };

        self.canvas_surfaces.insert(node_id, canvas);

        true
    }

    pub(super) fn canvas_fill_rect(
        &mut self,
        node_id: u64,
        receiver: Option<Value>,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        let Some(Value::Object(context)) = receiver else {
            return Err(JsError::type_error("fillRect: invalid receiver"));
        };

        if !self
            .canvas_contexts
            .get(&node_id)
            .is_some_and(|expected| std::rc::Rc::ptr_eq(expected, &context))
        {
            return Err(JsError::type_error("fillRect: invalid receiver"));
        }

        if arguments.len() < 4 {
            return Err(JsError::type_error("fillRect requires four arguments"));
        }

        let mut coordinates = [0.0; 4];

        for (output, value) in coordinates.iter_mut().zip(&arguments) {
            *output = self
                .to_number(value)
                .ok_or_else(|| JsError::type_error("fillRect: unsupported number conversion"))?;
        }

        let fill_style = context
            .borrow()
            .get("fillStyle")
            .cloned()
            .unwrap_or(Value::Undefined);

        let color = match fill_style {
            Value::String(value) if value.eq_ignore_ascii_case("transparent") => Color::TRANSPARENT,

            Value::String(value) => {
                let rgb = parse_color(&value).unwrap_or(0);

                Color::rgba((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255)
            }

            _ => Color::BLACK,
        };

        if self.ensure_canvas_surface(node_id) {
            let canvas = self
                .canvas_surfaces
                .get_mut(&node_id)
                .expect("surface was initialized");

            canvas.set_fill_style(color);
            canvas.fill_rect(
                coordinates[0],
                coordinates[1],
                coordinates[2],
                coordinates[3],
            );
        }

        Ok(Value::Undefined)
    }
}
