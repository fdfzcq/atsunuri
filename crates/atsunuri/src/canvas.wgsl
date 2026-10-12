struct View {
    // Window position of the canvas's top-left corner, in physical pixels.
    offset: vec2<f32>,
    // Physical pixels per canvas pixel.
    scale: f32,
    _pad0: f32,
    canvas_size: vec2<f32>,
    _pad1: vec2<f32>,
    // Color drawn around the canvas, already in the output's encoding.
    background: vec4<f32>,
};

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var canvas_texture: texture_2d<f32>;
@group(0) @binding(2) var canvas_sampler: sampler;

// A single triangle that covers the whole window.
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let p = (position.xy - view.offset) / view.scale;
    let color = textureSampleLevel(canvas_texture, canvas_sampler, p / view.canvas_size, 0.0);
    let inside = all(p >= vec2<f32>(0.0)) && all(p < view.canvas_size);
    return select(view.background, color, inside);
}
