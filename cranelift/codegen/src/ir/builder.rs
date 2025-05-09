//! Cranelift instruction builder.
//!
//! A `Builder` provides a convenient interface for inserting instructions into a Cranelift
//! function. Many of its methods are generated from the meta language instruction definitions.

use crate::ir;
use crate::ir::instructions::InstructionFormat;
use crate::ir::types;
use crate::ir::{BlockArg, Inst, Opcode, Type, Value};
use crate::ir::{DataFlowGraph, InstructionData};

/// Base trait for instruction builders.
///
/// The `InstBuilderBase` trait provides the basic functionality required by the methods of the
/// generated `InstBuilder` trait. These methods should not normally be used directly. Use the
/// methods in the `InstBuilder` trait instead.
///
/// Any data type that implements `InstBuilderBase` also gets all the methods of the `InstBuilder`
/// trait.
pub trait InstBuilderBase<'f>: Sized {
    /// Get an immutable reference to the data flow graph that will hold the constructed
    /// instructions.
    fn data_flow_graph(&self) -> &DataFlowGraph;
    /// Get a mutable reference to the data flow graph that will hold the constructed
    /// instructions.
    fn data_flow_graph_mut(&mut self) -> &mut DataFlowGraph;

    /// Insert an instruction and return a reference to it, consuming the builder.
    ///
    /// The result types may depend on a controlling type variable. For non-polymorphic
    /// instructions with multiple results, pass `INVALID` for the `ctrl_typevar` argument.
    fn build(self, data: InstructionData, ctrl_typevar: Type) -> (Inst, &'f mut DataFlowGraph);
}

// Include trait code generated by `cranelift-codegen/meta/src/gen_inst.rs`.
//
// This file defines the `InstBuilder` trait as an extension of `InstBuilderBase` with methods per
// instruction format and per opcode.
include!(concat!(env!("OUT_DIR"), "/inst_builder.rs"));

/// Any type implementing `InstBuilderBase` gets all the `InstBuilder` methods for free.
impl<'f, T: InstBuilderBase<'f>> InstBuilder<'f> for T {}

/// Base trait for instruction inserters.
///
/// This is an alternative base trait for an instruction builder to implement.
///
/// An instruction inserter can be adapted into an instruction builder by wrapping it in an
/// `InsertBuilder`. This provides some common functionality for instruction builders that insert
/// new instructions, as opposed to the `ReplaceBuilder` which overwrites existing instructions.
pub trait InstInserterBase<'f>: Sized {
    /// Get an immutable reference to the data flow graph.
    fn data_flow_graph(&self) -> &DataFlowGraph;

    /// Get a mutable reference to the data flow graph.
    fn data_flow_graph_mut(&mut self) -> &mut DataFlowGraph;

    /// Insert a new instruction which belongs to the DFG.
    fn insert_built_inst(self, inst: Inst) -> &'f mut DataFlowGraph;
}

use core::marker::PhantomData;

/// Builder that inserts an instruction at the current position.
///
/// An `InsertBuilder` is a wrapper for an `InstInserterBase` that turns it into an instruction
/// builder with some additional facilities for creating instructions that reuse existing values as
/// their results.
pub struct InsertBuilder<'f, IIB: InstInserterBase<'f>> {
    inserter: IIB,
    unused: PhantomData<&'f u32>,
}

impl<'f, IIB: InstInserterBase<'f>> InsertBuilder<'f, IIB> {
    /// Create a new builder which inserts instructions at `pos`.
    /// The `dfg` and `pos.layout` references should be from the same `Function`.
    pub fn new(inserter: IIB) -> Self {
        Self {
            inserter,
            unused: PhantomData,
        }
    }

    /// Reuse result values in `reuse`.
    ///
    /// Convert this builder into one that will reuse the provided result values instead of
    /// allocating new ones. The provided values for reuse must not be attached to anything. Any
    /// missing result values will be allocated as normal.
    ///
    /// The `reuse` argument is expected to be an array of `Option<Value>`.
    pub fn with_results<Array>(self, reuse: Array) -> InsertReuseBuilder<'f, IIB, Array>
    where
        Array: AsRef<[Option<Value>]>,
    {
        InsertReuseBuilder {
            inserter: self.inserter,
            reuse,
            unused: PhantomData,
        }
    }

    /// Reuse a single result value.
    ///
    /// Convert this into a builder that will reuse `v` as the single result value. The reused
    /// result value `v` must not be attached to anything.
    ///
    /// This method should only be used when building an instruction with exactly one result. Use
    /// `with_results()` for the more general case.
    pub fn with_result(self, v: Value) -> InsertReuseBuilder<'f, IIB, [Option<Value>; 1]> {
        // TODO: Specialize this to return a different builder that just attaches `v` instead of
        // calling `make_inst_results_reusing()`.
        self.with_results([Some(v)])
    }
}

impl<'f, IIB: InstInserterBase<'f>> InstBuilderBase<'f> for InsertBuilder<'f, IIB> {
    fn data_flow_graph(&self) -> &DataFlowGraph {
        self.inserter.data_flow_graph()
    }

    fn data_flow_graph_mut(&mut self) -> &mut DataFlowGraph {
        self.inserter.data_flow_graph_mut()
    }

    fn build(mut self, data: InstructionData, ctrl_typevar: Type) -> (Inst, &'f mut DataFlowGraph) {
        let inst;
        {
            let dfg = self.inserter.data_flow_graph_mut();
            inst = dfg.make_inst(data);
            dfg.make_inst_results(inst, ctrl_typevar);
        }
        (inst, self.inserter.insert_built_inst(inst))
    }
}

/// Builder that inserts a new instruction like `InsertBuilder`, but reusing result values.
pub struct InsertReuseBuilder<'f, IIB, Array>
where
    IIB: InstInserterBase<'f>,
    Array: AsRef<[Option<Value>]>,
{
    inserter: IIB,
    reuse: Array,
    unused: PhantomData<&'f u32>,
}

impl<'f, IIB, Array> InstBuilderBase<'f> for InsertReuseBuilder<'f, IIB, Array>
where
    IIB: InstInserterBase<'f>,
    Array: AsRef<[Option<Value>]>,
{
    fn data_flow_graph(&self) -> &DataFlowGraph {
        self.inserter.data_flow_graph()
    }

    fn data_flow_graph_mut(&mut self) -> &mut DataFlowGraph {
        self.inserter.data_flow_graph_mut()
    }

    fn build(mut self, data: InstructionData, ctrl_typevar: Type) -> (Inst, &'f mut DataFlowGraph) {
        let inst;
        {
            let dfg = self.inserter.data_flow_graph_mut();
            inst = dfg.make_inst(data);
            // Make an `Iterator<Item = Option<Value>>`.
            let ru = self.reuse.as_ref().iter().cloned();
            dfg.make_inst_results_reusing(inst, ctrl_typevar, ru);
        }
        (inst, self.inserter.insert_built_inst(inst))
    }
}

/// Instruction builder that replaces an existing instruction.
///
/// The inserted instruction will have the same `Inst` number as the old one.
///
/// If the old instruction still has result values attached, it is assumed that the new instruction
/// produces the same number and types of results. The old result values are preserved. If the
/// replacement instruction format does not support multiple results, the builder panics. It is a
/// bug to leave result values dangling.
pub struct ReplaceBuilder<'f> {
    dfg: &'f mut DataFlowGraph,
    inst: Inst,
}

impl<'f> ReplaceBuilder<'f> {
    /// Create a `ReplaceBuilder` that will overwrite `inst`.
    pub fn new(dfg: &'f mut DataFlowGraph, inst: Inst) -> Self {
        Self { dfg, inst }
    }
}

impl<'f> InstBuilderBase<'f> for ReplaceBuilder<'f> {
    fn data_flow_graph(&self) -> &DataFlowGraph {
        self.dfg
    }

    fn data_flow_graph_mut(&mut self) -> &mut DataFlowGraph {
        self.dfg
    }

    fn build(self, data: InstructionData, ctrl_typevar: Type) -> (Inst, &'f mut DataFlowGraph) {
        // Splat the new instruction on top of the old one.
        self.dfg.insts[self.inst] = data;

        if !self.dfg.has_results(self.inst) {
            // The old result values were either detached or non-existent.
            // Construct new ones.
            self.dfg.make_inst_results(self.inst, ctrl_typevar);
        }

        (self.inst, self.dfg)
    }
}

#[cfg(test)]
mod tests {
    use crate::cursor::{Cursor, FuncCursor};
    use crate::ir::condcodes::*;
    use crate::ir::types::*;
    use crate::ir::{Function, InstBuilder, ValueDef};

    #[test]
    fn types() {
        let mut func = Function::new();
        let block0 = func.dfg.make_block();
        let arg0 = func.dfg.append_block_param(block0, I32);
        let mut pos = FuncCursor::new(&mut func);
        pos.insert_block(block0);

        // Explicit types.
        let v0 = pos.ins().iconst(I32, 3);
        assert_eq!(pos.func.dfg.value_type(v0), I32);

        // Inferred from inputs.
        let v1 = pos.ins().iadd(arg0, v0);
        assert_eq!(pos.func.dfg.value_type(v1), I32);

        // Formula.
        let cmp = pos.ins().icmp(IntCC::Equal, arg0, v0);
        assert_eq!(pos.func.dfg.value_type(cmp), I8);
    }

    #[test]
    fn reuse_results() {
        let mut func = Function::new();
        let block0 = func.dfg.make_block();
        let arg0 = func.dfg.append_block_param(block0, I32);
        let mut pos = FuncCursor::new(&mut func);
        pos.insert_block(block0);

        let v0 = pos.ins().iadd_imm(arg0, 17);
        assert_eq!(pos.func.dfg.value_type(v0), I32);
        let iadd = pos.prev_inst().unwrap();
        assert_eq!(pos.func.dfg.value_def(v0), ValueDef::Result(iadd, 0));

        // Detach v0 and reuse it for a different instruction.
        pos.func.dfg.clear_results(iadd);
        let v0b = pos.ins().with_result(v0).iconst(I32, 3);
        assert_eq!(v0, v0b);
        assert_eq!(pos.current_inst(), Some(iadd));
        let iconst = pos.prev_inst().unwrap();
        assert!(iadd != iconst);
        assert_eq!(pos.func.dfg.value_def(v0), ValueDef::Result(iconst, 0));
    }

    #[test]
    #[should_panic]
    #[cfg(debug_assertions)]
    fn panics_when_inserting_wrong_opcode() {
        use crate::ir::{Opcode, TrapCode};

        let mut func = Function::new();
        let block0 = func.dfg.make_block();
        let mut pos = FuncCursor::new(&mut func);
        pos.insert_block(block0);

        // We are trying to create a Opcode::Return with the InstData::Trap, which is obviously wrong
        pos.ins()
            .Trap(Opcode::Return, I32, TrapCode::BAD_CONVERSION_TO_INTEGER);
    }
}
