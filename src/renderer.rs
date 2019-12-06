use vulkano::buffer::{BufferUsage, CpuAccessibleBuffer};
use vulkano::command_buffer::AutoCommandBufferBuilder;
use vulkano::descriptor::descriptor_set::{DescriptorSet, PersistentDescriptorSet};
use vulkano::descriptor::pipeline_layout::PipelineLayout;
use vulkano::device::{Device, Queue};
use vulkano::format::Format;
use vulkano::image::{Dimensions, StorageImage};
use vulkano::pipeline::ComputePipeline;

use std::sync::Arc;

use crate::shaders::{
    self, FinalizePushData, FinalizeShaderLayout, RandomizeShaderLayout, SimulateShaderLayout,
};

type RandomizePipeline = ComputePipeline<PipelineLayout<RandomizeShaderLayout>>;
type SimulatePipeline = ComputePipeline<PipelineLayout<SimulateShaderLayout>>;
type FinalizePipeline = ComputePipeline<PipelineLayout<FinalizeShaderLayout>>;

type GenericImage = StorageImage<Format>;
type GenericDescriptorSet = dyn DescriptorSet + Sync + Send;

const WORLD_SIZE: u32 = 1024;
const PARAMETER_SPACE: u32 = 128;

pub struct Renderer {
    target_width: u32,
    target_height: u32,
    reset_requested: bool,
    rate: u32,
    frame_step: u32,

    world_buffer_source: Arc<GenericImage>,
    world_buffer_target: Arc<GenericImage>,

    parameter_buffer: Arc<CpuAccessibleBuffer<[u16]>>,
    parameter_image: Arc<GenericImage>,

    randomize_pipeline: Arc<RandomizePipeline>,
    randomize_descriptors: Arc<GenericDescriptorSet>,

    simulate_pipeline: Arc<SimulatePipeline>,
    simulate_descriptors: Arc<GenericDescriptorSet>,

    finalize_push_data: FinalizePushData,
    finalize_pipeline: Arc<FinalizePipeline>,
    finalize_descriptors: Arc<GenericDescriptorSet>,
}

struct RenderBuilder {
    device: Arc<Device>,
    queue: Arc<Queue>,
    target_image: Arc<GenericImage>,
}

impl RenderBuilder {
    fn build(self) -> Renderer {
        let (target_width, target_height) = match self.target_image.dimensions() {
            Dimensions::Dim2d { width, height } => (width, height),
            _ => panic!("A non-2d image was passed as the target of a Renderer."),
        };

        let world_buffer_source = StorageImage::new(
            self.device.clone(),
            Dimensions::Dim2d {
                width: WORLD_SIZE,
                height: WORLD_SIZE,
            },
            Format::R16Uint,
            Some(self.queue.family()),
        )
        .unwrap();

        let world_buffer_target = StorageImage::new(
            self.device.clone(),
            Dimensions::Dim2d {
                width: WORLD_SIZE,
                height: WORLD_SIZE,
            },
            Format::R16Uint,
            Some(self.queue.family()),
        )
        .unwrap();

        let parameter_buffer = CpuAccessibleBuffer::from_iter(
            self.device.clone(),
            BufferUsage::all(),
            (0..PARAMETER_SPACE).map(|_| 0u16),
        )
        .unwrap();
        let parameter_image = StorageImage::new(
            self.device.clone(),
            Dimensions::Dim1d {
                width: PARAMETER_SPACE,
            },
            Format::R16Uint,
            Some(self.queue.family()),
        )
        .unwrap();

        let randomize_shader = shaders::load_randomize_shader(self.device.clone());
        let simulate_shader = shaders::load_simulate_shader(self.device.clone());
        let finalize_shader = shaders::load_finalize_shader(self.device.clone());

        let randomize_pipeline = Arc::new(
            ComputePipeline::new(
                self.device.clone(),
                &randomize_shader.main_entry_point(),
                &(),
            )
            .unwrap(),
        );
        let randomize_descriptors: Arc<GenericDescriptorSet> = Arc::new(
            PersistentDescriptorSet::start(randomize_pipeline.clone(), 0)
                .add_image(world_buffer_source.clone())
                .unwrap()
                .build()
                .unwrap(),
        );

        let simulate_pipeline = Arc::new(
            ComputePipeline::new(
                self.device.clone(),
                &simulate_shader.main_entry_point(),
                &(),
            )
            .unwrap(),
        );
        let simulate_descriptors: Arc<GenericDescriptorSet> = Arc::new(
            PersistentDescriptorSet::start(simulate_pipeline.clone(), 0)
                .add_image(world_buffer_source.clone())
                .unwrap()
                .add_image(world_buffer_target.clone())
                .unwrap()
                .add_image(parameter_image.clone())
                .unwrap()
                .build()
                .unwrap(),
        );

        let finalize_pipeline = Arc::new(
            ComputePipeline::new(
                self.device.clone(),
                &finalize_shader.main_entry_point(),
                &(),
            )
            .unwrap(),
        );
        let finalize_descriptors: Arc<GenericDescriptorSet> = Arc::new(
            PersistentDescriptorSet::start(finalize_pipeline.clone(), 0)
                .add_image(world_buffer_source.clone())
                .unwrap()
                .add_image(self.target_image.clone())
                .unwrap()
                .build()
                .unwrap(),
        );

        Renderer {
            target_width,
            target_height,
            reset_requested: true,
            rate: 1,
            frame_step: 0,

            randomize_pipeline,
            randomize_descriptors,

            parameter_buffer,
            parameter_image,

            world_buffer_source,
            world_buffer_target,

            simulate_pipeline,
            simulate_descriptors,

            finalize_push_data: FinalizePushData {
                offset: [0, 0],
                zoom: 1,
            },
            finalize_pipeline,
            finalize_descriptors,
        }
    }
}

impl Renderer {
    pub fn new(
        device: Arc<Device>,
        queue: Arc<Queue>,
        target_image: Arc<GenericImage>,
    ) -> Renderer {
        RenderBuilder {
            device,
            queue,
            target_image,
        }
        .build()
    }

    pub fn set_offset(&mut self, x: f32, y: f32) {
        self.finalize_push_data.offset[0] = (x * WORLD_SIZE as f32) as i32;
        self.finalize_push_data.offset[1] = (y * WORLD_SIZE as f32) as i32;
    }

    pub fn offset_zoom(&mut self, increment: bool) {
        if increment {
            self.finalize_push_data.zoom += 1;
        } else {
            self.finalize_push_data.zoom -= 1;
        }
        if self.finalize_push_data.zoom > 4 {
            self.finalize_push_data.zoom = 4;
        } else if self.finalize_push_data.zoom < 1 {
            self.finalize_push_data.zoom = 1;
        }
        println!("{}x zoom", self.finalize_push_data.zoom);
    }

    pub fn offset_rate(&mut self, increase: bool) {
        if increase {
            if self.rate == 0 {
                self.rate = 1;
            } else {
                self.rate *= 2;
            }
        } else {
            if self.rate > 1 {
                self.rate /= 2;
            } else {
                self.rate = 0;
            }
        }
        println!("{} generations per frame", self.rate);
    }

    pub fn pause(&mut self) {
        self.rate = 0;
    }

    pub fn skip_frames(&mut self, num_frames: u32) {
        self.frame_step += num_frames;
    }

    pub fn reset_world(&mut self) {
        self.reset_requested = true;
        self.frame_step += 1;
        println!("reset world");
    }

    pub fn set_parameters(&mut self, parameters: &Vec<i16>) {
        let mut destination = self.parameter_buffer.write().unwrap();
        for (index, parameter) in parameters.iter().enumerate() {
            destination[index] = *parameter as u16;
        }
    }

    pub fn add_render_commands(
        &mut self,
        mut add_to: AutoCommandBufferBuilder,
    ) -> AutoCommandBufferBuilder {
        add_to = add_to
            .copy_buffer_to_image(self.parameter_buffer.clone(), self.parameter_image.clone())
            .unwrap();
        if self.reset_requested {
            add_to = add_to
                .dispatch(
                    [WORLD_SIZE / 8, WORLD_SIZE / 8, 1],
                    self.randomize_pipeline.clone(),
                    self.randomize_descriptors.clone(),
                    (),
                )
                .unwrap();
            self.reset_requested = false;
        }
        for _ in 0..self.rate+self.frame_step {
            add_to = add_to
                .dispatch(
                    [WORLD_SIZE / 8, WORLD_SIZE / 8, 1],
                    self.simulate_pipeline.clone(),
                    self.simulate_descriptors.clone(),
                    (),
                )
                .unwrap()
                .copy_image(
                    self.world_buffer_target.clone(),
                    [0, 0, 0],
                    0,
                    0,
                    self.world_buffer_source.clone(),
                    [0, 0, 0],
                    0,
                    0,
                    [WORLD_SIZE, WORLD_SIZE, 1],
                    1,
                )
                .unwrap()
        }
        self.frame_step = 0;
        add_to
            .copy_image(
                self.world_buffer_target.clone(),
                [0, 0, 0],
                0,
                0,
                self.world_buffer_source.clone(),
                [0, 0, 0],
                0,
                0,
                [WORLD_SIZE, WORLD_SIZE, 1],
                1,
            )
            .unwrap()
            .dispatch(
                [self.target_width / 8, self.target_height / 8, 1],
                self.finalize_pipeline.clone(),
                self.finalize_descriptors.clone(),
                self.finalize_push_data.clone(),
            )
            .unwrap()
    }
}
