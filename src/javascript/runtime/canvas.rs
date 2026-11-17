//! Partial HTML canvas context bindings.
//!
//! <https://html.spec.whatwg.org/multipage/canvas.html#dom-canvas-getcontext-div>
//!
//! The context is an identity placeholder; drawing and the canvas bitmap are
//! not connected to this object yet.

use super::{Interpreter, JsError, Value};

impl Interpreter {
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

        let Value::Object(context) = Value::object_with_entries_and_prototype(
            [("canvas".to_owned(), Value::Element(node_id))],
            self.object_prototype.clone(),
        ) else {
            unreachable!("object constructor returns an object");
        };

        self.canvas_contexts.insert(node_id, context.clone());

        Ok(Value::Object(context))
    }
}
