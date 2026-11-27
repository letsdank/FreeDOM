//! Partial synchronous context bindings.
//!
//! <https://html.spec.whatwg.org/multipage/canvas.html#canvasrenderingcontext2d>

use super::{ElementMethod, HostRoots, Interpreter, JsError, ObjectStorage, RuntimeRoot, Value};
use crate::{MAX_PAGE_CANVAS_PIXELS, MAX_PAGE_CANVASES};
use freedom_canvas::{Canvas2D, parse_canvas_dimension};
use freedom_css::parse_color;
use freedom_graphics_core::{
    geometry::Transform,
    raster::{Color, FillRule, StrokeLineCap, StrokeLineJoin, StrokeStyle},
};
use std::collections::HashMap;

const CANVAS_SAVED_PROPERTIES: [&str; 7] = [
    "fillStyle",
    "strokeStyle",
    "lineWidth",
    "lineCap",
    "lineJoin",
    "miterLimit",
    "lineDashOffset",
];

const MAX_SAVED_CANVAS_STATES: usize = 4096;
const MAX_SAVED_CANVAS_DASH_VALUES: usize = 1_048_576;

/// Canvas state owned by the browser host for one interpreter session.
#[derive(Default)]
pub(super) struct CanvasHost {
    contexts: HashMap<u64, ObjectStorage>,
    surfaces: HashMap<u64, Canvas2D>,
    dashes: HashMap<u64, Vec<f64>>,
    state_stacks: HashMap<u64, Vec<CanvasSavedState>>,
    saved_states: usize,
    saved_dash_values: usize,
}

impl CanvasHost {
    pub(super) fn visit_roots(&self, visit: &mut impl FnMut(RuntimeRoot<'_>)) {
        for object in self.contexts.values() {
            visit(RuntimeRoot::Object(object));
        }

        for stack in self.state_stacks.values() {
            for state in stack {
                state.visit_roots(visit);
            }
        }
    }

    fn ensure_surface(&mut self, node_id: u64, width: usize, height: usize) -> bool {
        if let Some(existing) = self.surfaces.get(&node_id)
            && existing.surface().width() == width
            && existing.surface().height() == height
        {
            return true;
        }

        self.surfaces.remove(&node_id);

        let used: usize = self
            .surfaces
            .values()
            .map(|canvas| canvas.surface().width() * canvas.surface().height())
            .sum();

        let Some(total) = width
            .checked_mul(height)
            .and_then(|pixels| used.checked_add(pixels))
        else {
            return false;
        };

        if total > MAX_PAGE_CANVAS_PIXELS || self.surfaces.len() >= MAX_PAGE_CANVASES {
            return false;
        }

        let Some(canvas) = Canvas2D::new(width, height) else {
            return false;
        };

        self.surfaces.insert(node_id, canvas);
        true
    }

    fn save_state(
        &mut self,
        node_id: u64,
        context: &ObjectStorage,
        roots: &HostRoots,
    ) -> Result<(), JsError> {
        let dash_len = self.dashes.get(&node_id).map_or(0, Vec::len);

        if self.saved_states >= MAX_SAVED_CANVAS_STATES
            || dash_len > MAX_SAVED_CANVAS_DASH_VALUES - self.saved_dash_values
        {
            return Err(JsError::unsupported("canvas state stack limit exceeded"));
        }

        let properties = CANVAS_SAVED_PROPERTIES.map(|name| {
            roots
                .read_ordinary_own_property(context, name)
                .unwrap_or(Value::Undefined)
        });
        let dash = self.dashes.get(&node_id).cloned().unwrap_or_default();
        let transform = self
            .surfaces
            .get(&node_id)
            .map(Canvas2D::current_transform)
            .unwrap_or(Transform::IDENTITY);

        self.state_stacks
            .entry(node_id)
            .or_default()
            .push(CanvasSavedState {
                properties,
                dash,
                transform,
            });
        self.saved_states += 1;
        self.saved_dash_values += dash_len;
        Ok(())
    }

    fn restore_state(&mut self, node_id: u64, context: &ObjectStorage, roots: &HostRoots) {
        let Some(saved) = self.state_stacks.get_mut(&node_id).and_then(Vec::pop) else {
            return;
        };

        if self.state_stacks.get(&node_id).is_some_and(Vec::is_empty) {
            self.state_stacks.remove(&node_id);
        }
        self.saved_states -= 1;
        self.saved_dash_values -= saved.dash.len();

        if let Some(canvas) = self.surfaces.get_mut(&node_id) {
            canvas.restore_transform(saved.transform);
        }
        for (name, value) in CANVAS_SAVED_PROPERTIES.into_iter().zip(saved.properties) {
            roots.write_ordinary_property(context, name.to_owned(), value);
        }
        if saved.dash.is_empty() {
            self.dashes.remove(&node_id);
        } else {
            self.dashes.insert(node_id, saved.dash);
        }
    }

    fn clear_saved_states(&mut self, node_id: u64) {
        if let Some(stack) = self.state_stacks.remove(&node_id) {
            self.saved_states -= stack.len();
            self.saved_dash_values -= stack.iter().map(|state| state.dash.len()).sum::<usize>();
        }
    }

    fn reset_context(&mut self, node_id: u64, roots: &HostRoots) -> bool {
        let Some(context) = self.contexts.get(&node_id).cloned() else {
            return false;
        };

        self.surfaces.remove(&node_id);
        self.dashes.remove(&node_id);
        self.clear_saved_states(node_id);

        for (name, value) in [
            ("fillStyle", Value::String("#000000".to_owned())),
            ("strokeStyle", Value::String("#000000".to_owned())),
            ("lineWidth", Value::Number(1.0)),
            ("lineCap", Value::String("butt".to_owned())),
            ("lineJoin", Value::String("miter".to_owned())),
            ("miterLimit", Value::Number(10.0)),
            ("lineDashOffset", Value::Number(0.0)),
        ] {
            roots.write_ordinary_property(&context, name.to_owned(), value);
        }

        true
    }
}

struct CanvasSavedState {
    properties: [Value; 7],
    dash: Vec<f64>,
    transform: Transform,
}

impl CanvasSavedState {
    fn visit_roots(&self, visit: &mut impl FnMut(RuntimeRoot<'_>)) {
        for value in &self.properties {
            visit(RuntimeRoot::Value(value));
        }
    }
}

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
    pub(crate) fn take_canvas_surfaces(&mut self) -> std::collections::HashMap<u64, Canvas2D> {
        let ids: Vec<_> = self.canvas.surfaces.keys().copied().collect();

        for node_id in ids {
            self.ensure_canvas_surface(node_id);
        }

        std::mem::take(&mut self.canvas.surfaces)
    }

    pub(super) fn set_canvas_property(
        &mut self,
        object: &ObjectStorage,
        name: &str,
        value: &Value,
    ) -> Result<bool, JsError> {
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
            .canvas
            .contexts
            .values()
            .any(|context| self.host_roots.same_ordinary_object(context, object))
        {
            return Ok(false);
        }

        if (name == "lineCap" || name == "lineJoin")
            && let Value::String(value) = value
        {
            let valid = match name {
                "lineCap" => matches!(value.as_str(), "butt" | "round" | "square"),
                "lineJoin" => matches!(value.as_str(), "miter" | "round" | "bevel"),
                _ => false,
            };

            if valid {
                self.host_roots.write_ordinary_property(
                    object,
                    name.to_owned(),
                    Value::String(value.clone()),
                );
            }

            return Ok(true);
        }

        if name == "lineDashOffset" {
            let offset = self.to_number(value)?;
            if offset.is_finite() {
                self.host_roots.write_ordinary_property(
                    object,
                    name.to_owned(),
                    Value::Number(offset),
                );
            }

            return Ok(true);
        }

        if name == "lineWidth" || name == "miterLimit" {
            let number = self.to_number(value)?;
            let raster_number = number as f32;

            if number.is_finite()
                && number > 0.0
                && raster_number.is_finite()
                && raster_number > 0.0
            {
                self.host_roots.write_ordinary_property(
                    object,
                    name.to_owned(),
                    Value::Number(number),
                );
            }

            return Ok(true);
        }

        if let Value::String(value) = value {
            let value = value.trim();

            let normalized = if value.eq_ignore_ascii_case("transparent") {
                Some("transparent".to_owned())
            } else {
                parse_color(value).map(|rgb| format!("#{rgb:06x}"))
            };

            if let Some(normalized) = normalized {
                self.host_roots.write_ordinary_property(
                    object,
                    name.to_owned(),
                    Value::String(normalized),
                );
            }
        }

        Ok(true)
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

        if kind.to_js_string(&self.host_roots) != "2d" {
            return Ok(Value::Null);
        }

        if let Some(context) = self.canvas.contexts.get(&node_id) {
            return Ok(Value::Object(context.clone()));
        }

        if !self.ensure_canvas_surface(node_id) {
            return Ok(Value::Null);
        }

        let Value::Object(context) = self.host_roots.create_ordinary_object(
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
            super::ObjectPrototype::Object(self.object_prototype.clone()),
        ) else {
            unreachable!("object constructor returns an object");
        };

        self.host_roots.define_ordinary_property(
            &context,
            "fillRect".to_owned(),
            Value::ElementMethod(ElementMethod::CanvasFillRect(node_id)),
            false,
        );

        self.host_roots.define_ordinary_property(
            &context,
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
            ("save", ElementMethod::CanvasSave(node_id)),
            ("restore", ElementMethod::CanvasRestore(node_id)),
            ("translate", ElementMethod::CanvasTranslate(node_id)),
            ("rotate", ElementMethod::CanvasRotate(node_id)),
            ("scale", ElementMethod::CanvasScale(node_id)),
            ("transform", ElementMethod::CanvasTransform(node_id)),
            (
                "resetTransform",
                ElementMethod::CanvasResetTransform(node_id),
            ),
            ("setTransform", ElementMethod::CanvasSetTransform(node_id)),
        ] {
            self.host_roots.define_ordinary_property(
                &context,
                name.to_owned(),
                Value::ElementMethod(method),
                false,
            );
        }

        self.canvas.contexts.insert(node_id, context.clone());

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

        self.canvas.ensure_surface(node_id, width, height)
    }

    fn canvas_rect_arguments(
        &mut self,
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
            *output = self.to_number(value)?;
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

        let color = canvas_color(&self.host_roots, &context, "fillStyle");

        if self.ensure_canvas_surface(node_id) {
            let canvas = self
                .canvas
                .surfaces
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
                .canvas
                .surfaces
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
        if !matches!(name, "width" | "height")
            || !self.canvas.reset_context(node_id, &self.host_roots)
        {
            return;
        }

        self.ensure_canvas_surface(node_id);
    }

    pub(super) fn set_canvas_dimension(
        &mut self,
        node_id: u64,
        name: &str,
        value: Value,
    ) -> Result<(), JsError> {
        let value = self.webidl_unsigned_long(&value)?;

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
            .canvas
            .contexts
            .get(&node_id)
            .is_some_and(|expected| self.host_roots.same_ordinary_object(expected, &context))
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
            *target = self.to_number(value)?;
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
            &self.host_roots,
            &context,
            if matches!(operation, PathOperation::Stroke) {
                "strokeStyle"
            } else {
                "fillStyle"
            },
        );

        let stroke_style = if matches!(operation, PathOperation::Stroke) {
            Some(canvas_stroke_style(
                &self.host_roots,
                &context,
                self.canvas
                    .dashes
                    .get(&node_id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            )?)
        } else {
            None
        };

        if self.ensure_canvas_surface(node_id) {
            let canvas = self
                .canvas
                .surfaces
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
            let number = self.to_number(value)?;

            if !number.is_finite() || number < 0.0 {
                return Ok(Value::Undefined);
            }

            dash.push(number);
        }

        if dash.len() % 2 == 1 {
            dash.extend_from_within(..);
        }

        self.canvas.dashes.insert(node_id, dash);

        Ok(Value::Undefined)
    }

    pub(super) fn canvas_get_line_dash(
        &self,
        node_id: u64,
        receiver: Option<Value>,
    ) -> Result<Value, JsError> {
        self.canvas_context_receiver(node_id, receiver, "getLineDash")?;

        let values = self
            .canvas
            .dashes
            .get(&node_id)
            .map(|dash| dash.iter().copied().map(Value::Number).collect())
            .unwrap_or_default();

        Ok(Value::array(values))
    }

    // https://html.spec.whatwg.org/multipage/canvas.html#dom-context-2d-save-dev
    pub(super) fn canvas_save(
        &mut self,
        node_id: u64,
        receiver: Option<Value>,
    ) -> Result<Value, JsError> {
        let context = self.canvas_context_receiver(node_id, receiver, "save")?;

        self.canvas
            .save_state(node_id, &context, &self.host_roots)?;

        Ok(Value::Undefined)
    }

    // https://html.spec.whatwg.org/multipage/canvas.html#dom-context-2d-restore-dev
    pub(super) fn canvas_restore(
        &mut self,
        node_id: u64,
        receiver: Option<Value>,
    ) -> Result<Value, JsError> {
        let context = self.canvas_context_receiver(node_id, receiver, "restore")?;

        self.canvas
            .restore_state(node_id, &context, &self.host_roots);

        Ok(Value::Undefined)
    }

    pub(super) fn canvas_translate(
        &mut self,
        node_id: u64,
        receiver: Option<Value>,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        self.canvas_context_receiver(node_id, receiver, "translate")?;

        if arguments.len() < 2 {
            return Err(JsError::type_error("translate requires two arguments"));
        }

        let x = self.to_number(&arguments[0])?;
        let y = self.to_number(&arguments[1])?;

        if self.ensure_canvas_surface(node_id) {
            self.canvas
                .surfaces
                .get_mut(&node_id)
                .expect("surface was initialized")
                .translate(x, y);
        }

        Ok(Value::Undefined)
    }

    pub(super) fn canvas_rotate(
        &mut self,
        node_id: u64,
        receiver: Option<Value>,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        self.canvas_context_receiver(node_id, receiver, "rotate")?;

        let value = arguments
            .first()
            .ok_or_else(|| JsError::type_error("rotate requires an argument"))?;

        let angle = self.to_number(value)?;

        if self.ensure_canvas_surface(node_id) {
            self.canvas
                .surfaces
                .get_mut(&node_id)
                .expect("surface was initialized")
                .rotate(angle);
        }

        Ok(Value::Undefined)
    }

    pub(super) fn canvas_scale(
        &mut self,
        node_id: u64,
        receiver: Option<Value>,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        self.canvas_context_receiver(node_id, receiver, "scale")?;

        if arguments.len() < 2 {
            return Err(JsError::type_error("scale requires two arguments"));
        }

        let x = self.to_number(&arguments[0])?;
        let y = self.to_number(&arguments[1])?;

        if self.ensure_canvas_surface(node_id) {
            self.canvas
                .surfaces
                .get_mut(&node_id)
                .expect("surface was initialized")
                .scale(x, y);
        }

        Ok(Value::Undefined)
    }

    fn canvas_matrix_arguments(&mut self, arguments: &[Value]) -> Result<[f64; 6], JsError> {
        let mut values = [0.0; 6];

        for (output, value) in values.iter_mut().zip(arguments) {
            *output = self.to_number(value)?;
        }

        Ok(values)
    }

    pub(super) fn canvas_transform(
        &mut self,
        node_id: u64,
        receiver: Option<Value>,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        self.canvas_context_receiver(node_id, receiver, "transform")?;

        if arguments.len() < 6 {
            return Err(JsError::type_error("transform requires six arguments"));
        }

        let values = self.canvas_matrix_arguments(&arguments)?;

        if self.ensure_canvas_surface(node_id) {
            self.canvas
                .surfaces
                .get_mut(&node_id)
                .expect("surface was initialized")
                .transform(values);
        }

        Ok(Value::Undefined)
    }

    pub(super) fn canvas_reset_transform(
        &mut self,
        node_id: u64,
        receiver: Option<Value>,
    ) -> Result<Value, JsError> {
        self.canvas_context_receiver(node_id, receiver, "resetTransform")?;

        if self.ensure_canvas_surface(node_id) {
            self.canvas
                .surfaces
                .get_mut(&node_id)
                .expect("surface was initialized")
                .reset_transform();
        }

        Ok(Value::Undefined)
    }

    pub(super) fn canvas_set_transform(
        &mut self,
        node_id: u64,
        receiver: Option<Value>,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        self.canvas_context_receiver(node_id, receiver, "setTransform")?;

        let values = match arguments.as_slice() {
            [] | [Value::Undefined] | [Value::Null] => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],

            [Value::Object(_)] => {
                return Err(JsError::unsupported(
                    "setTransform: matrix dictionaries are not supported yet",
                ));
            }

            _ if arguments.len() >= 6 => self.canvas_matrix_arguments(&arguments)?,

            _ => {
                return Err(JsError::type_error(
                    "setTransform: requires zero, one object, or six arguments",
                ));
            }
        };

        if self.ensure_canvas_surface(node_id) {
            self.canvas
                .surfaces
                .get_mut(&node_id)
                .expect("surface was initialized")
                .set_transform(values);
        }

        Ok(Value::Undefined)
    }
}

fn canvas_stroke_style(
    owner: &HostRoots,
    context: &ObjectStorage,
    dash: &[f64],
) -> Result<StrokeStyle, JsError> {
    let line_cap = match owner.read_ordinary_own_property(context, "lineCap") {
        Some(Value::String(value)) if value == "round" => StrokeLineCap::Round,
        Some(Value::String(value)) if value == "square" => StrokeLineCap::Square,
        _ => StrokeLineCap::Butt,
    };

    let line_join = match owner.read_ordinary_own_property(context, "lineJoin") {
        Some(Value::String(value)) if value == "round" => StrokeLineJoin::Round,
        Some(Value::String(value)) if value == "bevel" => StrokeLineJoin::Bevel,
        _ => StrokeLineJoin::Miter,
    };

    let width = match owner.read_ordinary_own_property(context, "lineWidth") {
        Some(Value::Number(value)) => value as f32,
        _ => 1.0,
    };

    let miter_limit = match owner.read_ordinary_own_property(context, "miterLimit") {
        Some(Value::Number(value)) => value as f32,
        _ => 10.0,
    };

    let mut dash_array = Vec::with_capacity(dash.len());

    for &number in dash {
        let value = number as f32;

        if !value.is_finite() || (number != 0.0 && value < 0.0001) {
            return Err(JsError::unsupported(
                "stroke: tiny or oversized dash segment is not supported yet",
            ));
        }

        dash_array.push(value);
    }

    if !dash_array.iter().sum::<f32>().is_finite() {
        return Err(JsError::unsupported("stroke: dash pattern is too large"));
    }

    let offset = match owner.read_ordinary_own_property(context, "lineDashOffset") {
        Some(Value::Number(value)) => value,
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
    })
}

fn canvas_color(owner: &HostRoots, context: &ObjectStorage, property: &str) -> Color {
    match owner
        .read_ordinary_own_property(context, property)
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

#[cfg(test)]
mod tests {
    use super::{
        CanvasHost, CanvasSavedState, Color, HostRoots, Interpreter, MAX_PAGE_CANVAS_PIXELS,
        MAX_PAGE_CANVASES, MAX_SAVED_CANVAS_STATES, RuntimeRoot, Transform, Value,
    };

    #[test]
    fn canvas_host_reuses_resizes_and_limits_surfaces() {
        let mut host = CanvasHost::default();
        assert!(host.ensure_surface(1, 2, 1));
        host.surfaces
            .get_mut(&1)
            .unwrap()
            .fill_rect(0.0, 0.0, 1.0, 1.0);
        assert!(host.ensure_surface(1, 2, 1));
        assert_eq!(host.surfaces[&1].surface().pixel(0, 0), Some(Color::BLACK));

        assert!(host.ensure_surface(1, 1, 1));
        assert_eq!(
            host.surfaces[&1].surface().pixel(0, 0),
            Some(Color::TRANSPARENT)
        );
        assert!(!host.ensure_surface(1, usize::MAX, 2));
        assert!(!host.surfaces.contains_key(&1));
        assert!(!host.ensure_surface(1, MAX_PAGE_CANVAS_PIXELS + 1, 1));
        assert!(!host.ensure_surface(1, 16_385, 1));

        for node_id in 0..MAX_PAGE_CANVASES as u64 {
            assert!(host.ensure_surface(node_id, 0, 0));
        }
        assert!(host.ensure_surface(0, 0, 0));
        assert!(!host.ensure_surface(MAX_PAGE_CANVASES as u64, 0, 0));
    }

    #[test]
    fn saved_state_accounting_is_shared_and_cleared_per_canvas() {
        let roots = HostRoots::default();
        let Value::Object(context) = Value::object([]) else {
            unreachable!();
        };
        let mut host = CanvasHost::default();
        host.dashes.insert(1, vec![1.0, 2.0]);
        host.save_state(1, &context, &roots).unwrap();
        host.dashes.insert(1, vec![3.0, 4.0, 5.0]);
        host.save_state(1, &context, &roots).unwrap();
        host.dashes.insert(2, vec![7.0]);
        host.save_state(2, &context, &roots).unwrap();

        assert_eq!((host.saved_states, host.saved_dash_values), (3, 6));
        host.restore_state(1, &context, &roots);
        assert_eq!(host.dashes.get(&1).unwrap(), &[3.0, 4.0, 5.0]);
        assert_eq!((host.saved_states, host.saved_dash_values), (2, 3));
        host.clear_saved_states(1);
        assert_eq!((host.saved_states, host.saved_dash_values), (1, 1));
        host.restore_state(1, &context, &roots);
        assert_eq!((host.saved_states, host.saved_dash_values), (1, 1));
        host.restore_state(2, &context, &roots);
        assert_eq!((host.saved_states, host.saved_dash_values), (0, 0));

        for _ in 0..MAX_SAVED_CANVAS_STATES {
            host.save_state(3, &context, &roots).unwrap();
        }
        assert!(host.save_state(2, &context, &roots).is_err());
        assert_eq!(host.saved_states, MAX_SAVED_CANVAS_STATES);
        host.clear_saved_states(3);
        assert_eq!((host.saved_states, host.saved_dash_values), (0, 0));
    }

    #[test]
    fn canvas_host_keeps_contexts_and_saved_values_reachable() {
        let mut interpreter = Interpreter::new();
        let Value::Object(context) = Value::object([]) else {
            unreachable!();
        };
        let Value::Object(saved_value) = Value::object([]) else {
            unreachable!();
        };

        interpreter.canvas.contexts.insert(7, context.clone());
        interpreter.canvas.state_stacks.insert(
            7,
            vec![CanvasSavedState {
                properties: [
                    Value::Object(saved_value.clone()),
                    Value::Undefined,
                    Value::Undefined,
                    Value::Undefined,
                    Value::Undefined,
                    Value::Undefined,
                    Value::Undefined,
                ],
                dash: Vec::new(),
                transform: Transform::IDENTITY,
            }],
        );

        let mut saw_context = false;
        let mut saw_saved_value = false;
        let _snapshot = interpreter
            .visit_runtime_roots(&mut |root| match root {
                RuntimeRoot::Object(object) => {
                    saw_context |= interpreter
                        .host_roots
                        .same_ordinary_object(object, &context);
                }
                RuntimeRoot::Value(Value::Object(object)) => {
                    saw_saved_value |= interpreter
                        .host_roots
                        .same_ordinary_object(object, &saved_value);
                }
                _ => {}
            })
            .unwrap();

        assert!(saw_context);
        assert!(saw_saved_value);
    }
}
