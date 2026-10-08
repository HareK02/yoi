use wip_protocol::{Documentation, InterfaceReference, Object, TypeExpr};

use crate::{Interface, RenderError, ResourceLimit};

#[derive(Default)]
struct Budget {
    bytes: usize,
    nodes: usize,
}

fn check(value: usize, max: usize, limit: ResourceLimit) -> Result<(), RenderError> {
    if value > max {
        Err(RenderError::ResourceLimit { limit })
    } else {
        Ok(())
    }
}

impl Budget {
    fn bytes(&mut self, len: usize) -> Result<(), RenderError> {
        self.bytes = self.bytes.saturating_add(len);
        check(self.bytes, 1024 * 1024, ResourceLimit::InputBytes)
    }

    fn nodes(&mut self, len: usize) -> Result<(), RenderError> {
        self.nodes = self.nodes.saturating_add(len);
        check(self.nodes, 16_384, ResourceLimit::Nodes)
    }

    fn reference(&mut self, reference: &InterfaceReference) -> Result<(), RenderError> {
        self.nodes(1)?;
        self.bytes(reference.scope.len())?;
        self.bytes(reference.name.len())
    }

    fn doc(&mut self, doc: &Option<Documentation>) -> Result<(), RenderError> {
        if let Some(doc) = doc {
            self.bytes(doc.summary.len())?;
            self.bytes(doc.details.as_ref().map_or(0, String::len))?;
        }
        Ok(())
    }

    fn declaration(&mut self, name: &str, doc: &Option<Documentation>) -> Result<(), RenderError> {
        self.nodes(1)?;
        self.bytes(name.len())?;
        self.doc(doc)
    }

    fn expr(&mut self, expr: &TypeExpr, depth: usize) -> Result<(), RenderError> {
        check(depth, 64, ResourceLimit::TypeDepth)?;
        self.nodes(1)?;
        match expr {
            TypeExpr::Named { name } => self.bytes(name.len())?,
            TypeExpr::List { items } => self.expr(items, depth + 1)?,
            TypeExpr::Record { fields } => {
                self.nodes(fields.len())?;
                for field in fields {
                    self.bytes(field.name.len())?;
                    self.doc(&field.documentation)?;
                    self.expr(&field.r#type, depth + 1)?;
                }
            }
            TypeExpr::Enum { cases } => {
                self.nodes(cases.len())?;
                for case in cases {
                    self.bytes(case.name.len())?;
                    self.doc(&case.documentation)?;
                }
            }
            TypeExpr::Union { cases } => {
                self.nodes(cases.len())?;
                for case in cases {
                    self.bytes(case.name.len())?;
                    self.doc(&case.documentation)?;
                    if let Some(payload) = &case.payload {
                        self.expr(payload, depth + 1)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

pub(crate) fn object(object: &Object) -> Result<(), RenderError> {
    let mut budget = Budget::default();
    budget.bytes(object.name.len())?;
    budget.bytes(object.description.as_ref().map_or(0, String::len))?;
    budget.bytes(object.r#ref.as_ref().map_or(0, String::len))?;
    budget.bytes(object.validator.as_ref().map_or(0, Vec::len))?;
    check(object.interfaces.len(), 16_384, ResourceLimit::Nodes)?;
    for reference in &object.interfaces {
        budget.reference(reference)?;
    }
    Ok(())
}

pub(crate) fn interface(interface: &Interface<'_>) -> Result<(), RenderError> {
    let mut budget = Budget::default();
    let descriptor = interface.descriptor;
    budget.reference(interface.reference)?;
    budget.bytes(descriptor.format.len())?;
    budget.doc(&descriptor.documentation)?;
    check(descriptor.types.len(), 256, ResourceLimit::TypeDeclarations)?;
    // Check collection counts before traversing even empty names/expressions.
    check(descriptor.operations.len(), 16_384, ResourceLimit::Nodes)?;
    for ty in &descriptor.types {
        budget.declaration(&ty.name, &ty.documentation)?;
        budget.expr(&ty.definition, 1)?;
    }
    for operation in &descriptor.operations {
        budget.declaration(&operation.name, &operation.documentation)?;
        check(operation.parameters.len(), 16_384, ResourceLimit::Nodes)?;
        for parameter in &operation.parameters {
            budget.declaration(&parameter.name, &parameter.documentation)?;
            budget.expr(&parameter.r#type, 1)?;
        }
        budget.nodes(1)?; // Return declaration.
        budget.doc(&operation.returns.documentation)?;
        budget.expr(&operation.returns.r#type, 1)?;
    }
    Ok(())
}
