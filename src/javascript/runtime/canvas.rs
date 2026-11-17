//! Partial synchronous context bindings.
//!
//! <https://html.spec.whatwg.org/multipage/canvas.html#canvasrenderingcontext2d>

use super::{ElementMethod, Interpreter, JsError, ObjectStorage, Value};
use crate::{
    css::parse_color,
    graphics::{
        canvas::{Canvas2D, MAX_PAGE_CANVAS_PIXELS, MAX_PAGE_CANVASES, parse_canvas_dimension},
        raster::{Color, FillRule, StrokeLineCap, StrokeLineJoin, StrokeStyle},
    },
};

#[derive(Clone, Copy)]
pub(super) enum PathOperation {
    Begin,
    MoveTo,
    LineTo,
    Close,
    Fill,
    QuadraticCurveTo,
    BezierCurveTo,
    Stroke,
}

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
        if !matches!(
            name,
            "fillStyle"
                | "strokeStyle"
                | "lineWidth"
                | "lineCap"
                | "lineJoin"
                | "miterLimit"
                | "lineDashOffset"
        ) || !self
            .canvas_contexts
            .values()
            .any(|context| std::rc::Rc::ptr_eq(context, object))
        {
            return false;
        }

        if name == "lineCap" || name == "lineJoin" {
            if let Value::String(value) = value {
                let valid = match name {
                    "lineCap" => matches!(value.as_str(), "butt" | "round" | "square"),
                    "lineJoin" => matches!(value.as_str(), "miter" | "round" | "bevel"),
                    _ => false,
                };

                if valid {
                    object
                        .borrow_mut()
                        .set(name.to_owned(), Value::String(value.clone()));
                }

                return true;
            }
        }

        if name == "lineDashOffset" {
            if let Some(offset) = self.to_number(value).filter(|number| number.is_finite()) {
                object
                    .borrow_mut()
                    .set(name.to_owned(), Value::Number(offset));
            }

            return true;
        }

        if name == "lineWidth" || name == "miterLimit" {
            if let Some(number) = self.to_number(value) {
                let raster_number = number as f32;

                if number.is_finite()
                    && number > 0.0
                    && raster_number.is_finite()
                    && raster_number > 0.0
                {
                    object
                        .borrow_mut()
                        .set(name.to_owned(), Value::Number(number));
                }
            }

            return true;
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
                    .set(name.to_owned(), Value::String(normalized));
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
                (
                    "strokeStyle".to_owned(),
                    Value::String("#000000".to_owned()),
                ),
                ("lineWidth".to_owned(), Value::Number(1.0)),
                ("lineCap".to_owned(), Value::String("butt".to_owned())),
                ("lineJoin".to_owned(), Value::String("miter".to_owned())),
                ("miterLimit".to_owned(), Value::Number(10.0)),
                ("lineDashOffset".to_owned(), Value::Number(0.0)),
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

        context.borrow_mut().define_own_data_property(
            "clearRect".to_owned(),
            Value::ElementMethod(ElementMethod::CanvasClearRect(node_id)),
            false,
        );

        for (name, method) in [
            ("beginPath", ElementMethod::CanvasBeginPath(node_id)),
            ("moveTo", ElementMethod::CanvasMoveTo(node_id)),
            ("lineTo", ElementMethod::CanvasLineTo(node_id)),
            ("closePath", ElementMethod::CanvasClosePath(node_id)),
            ("fill", ElementMethod::CanvasFill(node_id)),
            ("stroke", ElementMethod::CanvasStroke(node_id)),
            (
                "quadraticCurveTo",
                ElementMethod::CanvasQuadraticCurveTo(node_id),
            ),
            ("bezierCurveTo", ElementMethod::CanvasBezierCurveTo(node_id)),
            ("setLineDash", ElementMethod::CanvasSetLineDash(node_id)),
            ("getLineDash", ElementMethod::CanvasGetLineDash(node_id)),
        ] {
            context.borrow_mut().define_own_data_property(
                name.to_owned(),
                Value::ElementMethod(method),
                false,
            );
        }

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

    fn canvas_rect_arguments(
        &self,
        node_id: u64,
        receiver: Option<Value>,
        arguments: Vec<Value>,
        method: &str,
    ) -> Result<(ObjectStorage, [f64; 4]), JsError> {
        let context = self.canvas_context_receiver(node_id, receiver, method)?;

        if arguments.len() < 4 {
            return Err(JsError::type_error(format!(
                "{method} requires four arguments"
            )));
        }

        let mut coordinates = [0.0; 4];

        for (output, value) in coordinates.iter_mut().zip(&arguments) {
            *output = self.to_number(value).ok_or_else(|| {
                JsError::type_error(format!("{method}: unsupported number conversion"))
            })?;
        }

        Ok((context, coordinates))
    }

    pub(super) fn canvas_fill_rect(
        &mut self,
        node_id: u64,
        receiver: Option<Value>,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        let (context, coordinates) =
            self.canvas_rect_arguments(node_id, receiver, arguments, "fillRect")?;

        let color = canvas_color(&context, "fillStyle");

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

    pub(super) fn canvas_clear_rect(
        &mut self,
        node_id: u64,
        receiver: Option<Value>,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        let (_, coordinates) =
            self.canvas_rect_arguments(node_id, receiver, arguments, "clearRect")?;

        if self.ensure_canvas_surface(node_id) {
            let canvas = self
                .canvas_surfaces
                .get_mut(&node_id)
                .expect("surface was initialized");

            canvas.clear_rect(
                coordinates[0],
                coordinates[1],
                coordinates[2],
                coordinates[3],
            );
        }

        Ok(Value::Undefined)
    }

    // https://html.spec.whatwg.org/multipage/canvas.html#the-canvas-element
    pub(super) fn reset_canvas_attribute(&mut self, node_id: u64, name: &str) {
        if !matches!(name, "width" | "height") || !self.canvas_contexts.contains_key(&node_id) {
            return;
        }

        self.canvas_surfaces.remove(&node_id);
        self.canvas_dashes.remove(&node_id);

        if let Some(context) = self.canvas_contexts.get(&node_id) {
            let mut context = context.borrow_mut();

            context.set("fillStyle".to_owned(), Value::String("#000000".to_owned()));

            context.set(
                "strokeStyle".to_owned(),
                Value::String("#000000".to_owned()),
            );

            context.set("lineWidth".to_owned(), Value::Number(1.0));
            context.set("lineCap".to_owned(), Value::String("butt".to_owned()));
            context.set("lineJoin".to_owned(), Value::String("miter".to_owned()));
            context.set("miterLimit".to_owned(), Value::Number(10.0));
            context.set("lineDashOffset".to_owned(), Value::Number(0.0));
        }

        self.ensure_canvas_surface(node_id);
    }

    pub(super) fn set_canvas_dimension(
        &mut self,
        node_id: u64,
        name: &str,
        value: Value,
    ) -> Result<(), JsError> {
        let number = self.to_number(&value).ok_or_else(|| {
            JsError::type_error("canvas dimension: unsupported number conversion")
        })?;

        let value = if number.is_finite() {
            number.trunc().rem_euclid(4_294_967_296.0) as u32
        } else {
            0
        };

        self.set_document_element_attribute(
            node_id,
            vec![
                Value::String(name.to_owned()),
                Value::String(value.to_string()),
            ],
        )?;

        Ok(())
    }

    fn canvas_context_receiver(
        &self,
        node_id: u64,
        receiver: Option<Value>,
        method: &str,
    ) -> Result<ObjectStorage, JsError> {
        let Some(Value::Object(context)) = receiver else {
            return Err(JsError::type_error(format!("{method}: invalid receiver")));
        };

        if !self
            .canvas_contexts
            .get(&node_id)
            .is_some_and(|expected| std::rc::Rc::ptr_eq(expected, &context))
        {
            return Err(JsError::type_error(format!("{method}: invalid receiver")));
        }

        Ok(context)
    }

    pub(super) fn canvas_path_operation(
        &mut self,
        node_id: u64,
        receiver: Option<Value>,
        arguments: Vec<Value>,
        operation: PathOperation,
    ) -> Result<Value, JsError> {
        let name = match operation {
            PathOperation::Begin => "beginPath",
            PathOperation::MoveTo => "moveTo",
            PathOperation::LineTo => "lineTo",
            PathOperation::Close => "closePath",
            PathOperation::Fill => "fill",
            PathOperation::Stroke => "stroke",
            PathOperation::QuadraticCurveTo => "quadraticCurveTo",
            PathOperation::BezierCurveTo => "bezierCurveTo",
        };

        let context = self.canvas_context_receiver(node_id, receiver, name)?;

        if matches!(operation, PathOperation::Stroke) && !arguments.is_empty() {
            return Err(JsError::type_error("stroke: Path2D is not supported yet"));
        }

        let coordinate_count = match operation {
            PathOperation::MoveTo | PathOperation::LineTo => 2,
            PathOperation::QuadraticCurveTo => 4,
            PathOperation::BezierCurveTo => 6,
            _ => 0,
        };

        if arguments.len() < coordinate_count {
            return Err(JsError::type_error(format!(
                "{name} requires {coordinate_count} arguments"
            )));
        }

        let mut coordinates = [0.0; 6];

        for (target, value) in coordinates
            .iter_mut()
            .zip(arguments.iter())
            .take(coordinate_count)
        {
            *target = self.to_number(value).ok_or_else(|| {
                JsError::type_error(format!("{name}: unsupported number conversion"))
            })?;
        }

        let rule = if matches!(operation, PathOperation::Fill) {
            match arguments.first() {
                None | Some(Value::Undefined) => FillRule::NonZero,

                Some(Value::String(value)) if value == "nonzero" => FillRule::NonZero,

                Some(Value::String(value)) if value == "evenodd" => FillRule::EvenOdd,

                _ => {
                    return Err(JsError::type_error("fill: unsupported fill rule or Path2D"));
                }
            }
        } else {
            FillRule::NonZero
        };

        let color = canvas_color(
            &context,
            if matches!(operation, PathOperation::Stroke) {
                "strokeStyle"
            } else {
                "fillStyle"
            },
        );

        let stroke_style = if matches!(operation, PathOperation::Stroke) {
            Some(canvas_stroke_style(
                &context,
                self.canvas_dashes
                    .get(&node_id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            )?)
        } else {
            None
        };

        if self.ensure_canvas_surface(node_id) {
            let canvas = self
                .canvas_surfaces
                .get_mut(&node_id)
                .expect("surface was initialized");

            match operation {
                PathOperation::Begin => canvas.begin_path(),

                PathOperation::MoveTo => {
                    canvas.move_to(coordinates[0], coordinates[1]);
                }

                PathOperation::LineTo => {
                    canvas.line_to(coordinates[0], coordinates[1]);
                }

                PathOperation::QuadraticCurveTo => {
                    canvas.quadratic_curve_to(
                        coordinates[0],
                        coordinates[1],
                        coordinates[2],
                        coordinates[3],
                    );
                }

                PathOperation::BezierCurveTo => {
                    canvas.bezier_curve_to(
                        coordinates[0],
                        coordinates[1],
                        coordinates[2],
                        coordinates[3],
                        coordinates[4],
                        coordinates[5],
                    );
                }

                PathOperation::Close => canvas.close_path(),

                PathOperation::Fill => {
                    canvas.set_fill_style(color);
                    canvas.fill(rule);
                }

                PathOperation::Stroke => {
                    canvas.stroke(color, stroke_style.expect("stroke style was initialized"))
                }
            }
        }

        Ok(Value::Undefined)
    }

    // https://html.spec.whatwg.org/multipage/canvas.html#dom-context-2d-setlinedash-dev
    pub(super) fn canvas_set_line_dash(
        &mut self,
        node_id: u64,
        receiver: Option<Value>,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        self.canvas_context_receiver(node_id, receiver, "setLineDash")?;

        let Some(value @ Value::Array(_)) = arguments.first() else {
            return Err(JsError::type_error(
                "setLineDash currently requires an array",
            ));
        };

        let values = self.create_list_from_array_like(value)?;

        if values.len() > 4096 {
            return Err(JsError::unsupported("setLineDash array limit exceeded"));
        }

        let mut dash = Vec::with_capacity(values.len().saturating_mul(2));

        for value in &values {
            let number = self.to_number(value).ok_or_else(|| {
                JsError::unsupported("setLineDash: unsupported number conversion")
            })?;

            if !number.is_finite() || number < 0.0 {
                return Ok(Value::Undefined);
            }

            dash.push(number);
        }

        if dash.len() % 2 == 1 {
            dash.extend_from_within(..);
        }

        self.canvas_dashes.insert(node_id, dash);

        Ok(Value::Undefined)
    }

    pub(super) fn canvas_get_line_dash(
        &self,
        node_id: u64,
        receiver: Option<Value>,
    ) -> Result<Value, JsError> {
        self.canvas_context_receiver(node_id, receiver, "getLineDash")?;

        let values = self
            .canvas_dashes
            .get(&node_id)
            .map(|dash| dash.iter().copied().map(Value::Number).collect())
            .unwrap_or_default();

        Ok(Value::array(values))
    }
}

fn canvas_stroke_style(context: &ObjectStorage, dash: &[f64]) -> Result<StrokeStyle, JsError> {
    let context = context.borrow();

    let line_cap = match context.get("lineCap") {
        Some(Value::String(value)) if value == "round" => StrokeLineCap::Round,
        Some(Value::String(value)) if value == "square" => StrokeLineCap::Square,
        _ => StrokeLineCap::Butt,
    };

    let line_join = match context.get("lineJoin") {
        Some(Value::String(value)) if value == "round" => StrokeLineJoin::Round,
        Some(Value::String(value)) if value == "bevel" => StrokeLineJoin::Bevel,
        _ => StrokeLineJoin::Miter,
    };

    let width = match context.get("lineWidth") {
        Some(Value::Number(value)) => *value as f32,
        _ => 1.0,
    };

    let miter_limit = match context.get("miterLimit") {
        Some(Value::Number(value)) => *value as f32,
        _ => 10.0,
    };

    let mut dash_array = Vec::with_capacity(dash.len());

    for &number in dash {
        let value = number as f32;

        if !value.is_finite() || value < 0.0001 {
            return Err(JsError::unsupported(
                "stroke: zero, tiny, or oversized dash segment is not supported yet",
            ));
        }

        dash_array.push(value);
    }

    if !dash_array.iter().sum::<f32>().is_finite() {
        return Err(JsError::unsupported("stroke: dash pattern is too large"));
    }

    let offset = match context.get("lineDashOffset") {
        Some(Value::Number(value)) => *value,
        _ => 0.0,
    };

    let dash_offset = offset as f32;

    if !dash_offset.is_finite() || (offset != 0.0 && dash_offset == 0.0) {
        return Err(JsError::unsupported(
            "stroke: dash offset is outside raster range",
        ));
    }

    Ok(StrokeStyle {
        width,
        line_cap,
        line_join,
        miter_limit,
        dash_array,
        dash_offset,
        ..StrokeStyle::default()
    })
}

fn canvas_color(context: &ObjectStorage, property: &str) -> Color {
    match context
        .borrow()
        .get(property)
        .cloned()
        .unwrap_or(Value::Undefined)
    {
        Value::String(value) if value.eq_ignore_ascii_case("transparent") => Color::TRANSPARENT,

        Value::String(value) => {
            let rgb = parse_color(&value).unwrap_or(0);

            Color::rgba((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255)
        }

        _ => Color::BLACK,
    }
}
