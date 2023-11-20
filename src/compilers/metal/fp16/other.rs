use std::sync::Arc;

use half::f16;
use petgraph::stable_graph::NodeIndex;

use crate::{
    op::{InputTensor, Operator},
    compilers::metal::*,
    prelude::*,
};

use metal_rs::{objc::rc::autoreleasepool, *};

/// Special kernel for efficient mean reduction
#[derive(Debug, Clone)]
pub struct MetalCos(ComputePipelineState, Device);
impl PartialEq for MetalCos {
    fn eq(&self, _: &Self) -> bool {
        false
    }
}

impl MetalCos {
    fn new(dev: Device) -> Self {
        let mut code = 
            "
#include <metal_stdlib>
using namespace metal;
kernel void mkernel(device half *inp [[buffer(0)]], device half *out [[buffer(1)]], device uint& n_elements [[buffer(2)]], uint i_ [[thread_position_in_grid]]) {{
    if (i_ < n_elements) {{
        out[i_] = cos(inp[i_]);
    }}
}}
".to_string();
        code = code.replace("mkernel", "kernel_metal_cos");

        Self(compile_function("kernel_metal_cos", &code, &dev), dev)
    }
}

impl MetalKernelForward for MetalCos {
    fn metal_forward(
            &self,
            inputs: &[(&Buffer, ShapeTracker)],
            dev: &Device,
            command_buffer: &CommandBufferRef,
        ) -> Vec<Buffer> {
        let inp_size = inputs[0].1.n_physical_elements();
        let encoder = command_buffer
            .compute_command_encoder_with_descriptor(ComputePassDescriptor::new());
        encoder.set_compute_pipeline_state(&self.0);

        let out = dev.new_buffer(
            (inp_size * std::mem::size_of::<f16>()) as u64,
            MTLResourceOptions::StorageModeManaged,
        );

        // Set inputs
        encoder.set_buffer(0, Some(inputs[0].0), 0);
        encoder.set_buffer(1, Some(&out), 0);
        encoder.set_int(2, inp_size as u32);

        // Execute
        encoder.dispatch_n_elements(inp_size);
        encoder.end_encoding();

        vec![out]
    }
}

impl Operator for MetalCos {
    fn process(&self, tensors: Vec<(InputTensor, ShapeTracker)>) -> Vec<Tensor> {
        autoreleasepool(|| {
            let a = tensors[0]
                .0
                .borrowed()
                .data
                .as_any()
                .downcast_ref::<Buffer>()
                .unwrap();
            // Setup command queue / command buffer / encoder
            let command_queue = self.1.new_command_queue();
            let command_buffer = command_queue.new_command_buffer();

            let out = self.metal_forward(&[(a, tensors[0].1)], &self.1, command_buffer).pop().unwrap();
            
            command_buffer.commit();
            command_buffer.wait_until_completed();

            vec![Tensor {
                data: Box::new(out),
            }]
        })
    }

    fn custom(&self, key: &str) -> Option<Box<dyn Any>> {
        if key == "metal" {
            return Some(Box::new(MetalKernelWrapper(Arc::new(Box::new(
                self.clone(),
            )))));
        }
        None
    }
}

/// Replace the mean reduce pattern with a special kernel. This is meant to be ran **after** the FakeSumReduceCompiler.
#[derive(Default, Debug)]
pub struct MetalCosCompiler;

impl Compiler for MetalCosCompiler {
    fn compile(&self, graph: &mut Graph) {
        let dev = Device::system_default().unwrap();
        // Look for the cos pattern
        // sin(add(mul(const_neg_one, x), const_pi_over_2))
        let (
            mut const_neg_one,
            mut const_pi,
            mut mul,
            mut add,
            mut sin,
            mut x,
        ) = (
            NodeIndex::default(),
            NodeIndex::default(),
            NodeIndex::default(),
            NodeIndex::default(),
            NodeIndex::default(),
            NodeIndex::default(),
        );

        let s = SelectEdge::new(
            SelectEdge::new(
                SelectOp::new().check(|op, _| if let Some(c) = op.as_any().downcast_ref::<MetalConstant<f16>>() {
                    c.0 == f16::PI / f16::from_f32(2.)
                } else {false}).ptr(&mut const_pi),
                SelectEdge::new(
                    SelectEdge::new(
                        SelectOp::new().ptr(&mut x),
                        SelectEdge::new(
                                SelectOp::new().check(|op, _| if let Some(c) = op.as_any().downcast_ref::<MetalConstant<f16>>() {
                                    c.0 == f16::NEG_ONE
                                } else {false}).ptr(&mut const_neg_one),
                            SelectOp::new().ty::<MetalMul<f16>>().ptr(&mut mul),
                        ),
                    ),
                    SelectOp::new().ty::<MetalAdd<f16>>().ptr(&mut add),
                ),
            ),
            SelectOp::new().ty::<MetalSin<f16>>().ptr(&mut sin),
        );
        for _ in s.search(graph) {
            if graph.no_delete.contains(&const_neg_one)
                || graph.no_delete.contains(&const_pi)
                || graph.no_delete.contains(&mul)
                || graph.no_delete.contains(&add)
            {
                // An intermediate node can't be deleted
                continue;
            }

            // Insert cos op
            let shape = graph.graph.edges_directed(mul, petgraph::Direction::Incoming).find_map(|e| e.weight().as_data()).unwrap().2;
            let cos = graph
                .add_op(MetalCos::new(dev.clone()))
                .input(x, 0, shape)
                .finish();

            // Create edges to dests
            move_outgoing_edge(sin, cos, &mut graph.graph);
            move_references(
                &mut graph.id_remap,
                &mut graph.no_delete,
                &mut graph.to_retrieve,
                sin,
                cos,
            );

            // Remove the old ops
            graph.graph.remove_node(mul);
            graph.graph.remove_node(add);
            graph.graph.remove_node(const_neg_one);
            graph.graph.remove_node(const_pi);
            graph.graph.remove_node(sin);
        }
    }
}

/// Special kernel for efficient mean reduction
#[derive(Debug, Clone)]
pub struct MetalExp(ComputePipelineState, Device);
impl PartialEq for MetalExp {
    fn eq(&self, _: &Self) -> bool {
        false
    }
}

impl MetalExp {
    fn new(dev: Device) -> Self {
        let mut code = 
            "
#include <metal_stdlib>
using namespace metal;
kernel void mkernel(device half *inp [[buffer(0)]], device half *out [[buffer(1)]], device uint& n_elements [[buffer(2)]], uint i_ [[thread_position_in_grid]]) {{
    if (i_ < n_elements) {{
        out[i_] = exp(inp[i_]);
    }}
}}
".to_string();
        code = code.replace("mkernel", "kernel_metal_exp");

        Self(compile_function("kernel_metal_exp", &code, &dev), dev)
    }
}

impl MetalKernelForward for MetalExp {
    fn metal_forward(
            &self,
            inputs: &[(&Buffer, ShapeTracker)],
            dev: &Device,
            command_buffer: &CommandBufferRef,
        ) -> Vec<Buffer> {
        let inp_size = inputs[0].1.n_physical_elements();

        let out = dev.new_buffer(
            (inp_size * std::mem::size_of::<f16>()) as u64,
            MTLResourceOptions::StorageModeManaged,
        );

        let encoder = command_buffer
                .compute_command_encoder_with_descriptor(ComputePassDescriptor::new());
            encoder.set_compute_pipeline_state(&self.0);

            // Set inputs
            encoder.set_buffer(0, Some(inputs[0].0), 0);
            encoder.set_buffer(1, Some(&out), 0);
            encoder.set_int(2, inp_size as u32);

            // Execute
            encoder.dispatch_n_elements(inp_size);
            encoder.end_encoding();

            vec![out]
    }
}

impl Operator for MetalExp {
    fn process(&self, tensors: Vec<(InputTensor, ShapeTracker)>) -> Vec<Tensor> {
        autoreleasepool(|| {
            // Setup buffers
            let a_inp = tensors[0]
                .0
                .borrowed()
                .data
                .as_any()
                .downcast_ref::<Buffer>()
                .unwrap();
            

            // Setup command queue / command buffer / encoder
            let command_queue = self.1.new_command_queue();
            let command_buffer = command_queue.new_command_buffer();

            let out = self.metal_forward(&[(a_inp, tensors[0].1)], &self.1, command_buffer).pop().unwrap();
            
            command_buffer.commit();
            command_buffer.wait_until_completed();

            vec![Tensor {
                data: Box::new(out),
            }]
        })
    }

    fn custom(&self, key: &str) -> Option<Box<dyn Any>> {
        if key == "metal" {
            return Some(Box::new(MetalKernelWrapper(Arc::new(Box::new(
                self.clone(),
            )))));
        }
        None
    }
}

/// Replace the mean reduce pattern with a special kernel. This is meant to be ran **after** the FakeSumReduceCompiler.
#[derive(Default, Debug)]
pub struct MetalExpCompiler;

impl Compiler for MetalExpCompiler {
    fn compile(&self, graph: &mut Graph) {
        let dev = Device::system_default().unwrap();
        // Look for the exp pattern
        // exp2(mul(x, const))
        let (
            mut constant,
            mut mul,
            mut exp2
        ) = (
            NodeIndex::default(),
            NodeIndex::default(),
            NodeIndex::default(),
        );

        let s = 
        SelectEdge::new(
            SelectEdge::new(
                SelectOp::new()
                    .check(|op, _| if let Some(c) = op.as_any().downcast_ref::<MetalConstant<f16>>() {
                            c.0 == f16::from_f32(1.0 / f32::ln(2.))
                        } else {false}
                    ).ptr(&mut constant), 
                SelectOp::new().ty::<MetalMul<f16>>().ptr(&mut mul)), 
            SelectOp::new().ty::<MetalExp2<f16>>().ptr(&mut exp2)
        );

        for _ in s.search(graph) {
            if graph.no_delete.contains(&constant)
                || graph.no_delete.contains(&mul)
                || graph.no_delete.contains(&exp2)
            {
                // An intermediate node can't be deleted
                continue;
            }

            // Insert exp op
            let src = graph.get_sources(mul).into_iter().find(|(i, _)| *i != constant).unwrap();
            let exp = graph
                .add_op(MetalExp::new(dev.clone()))
                .input(src.0, 0, src.1)
                .finish();

            // Create edges to dests
            move_outgoing_edge(exp2, exp, &mut graph.graph);
            move_references(
                &mut graph.id_remap,
                &mut graph.no_delete,
                &mut graph.to_retrieve,
                exp2,
                exp,
            );

            // Remove the old ops
            graph.graph.remove_node(mul);
            graph.graph.remove_node(constant);
            graph.graph.remove_node(exp2);
        }
    }
}