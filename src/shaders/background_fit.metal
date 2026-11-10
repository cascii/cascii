#include <metal_stdlib>
using namespace metal;
#pragma clang fp contract(off)

// Host layout: four floats followed by four uints, 32 bytes per glyph.
struct Glyph {
    float aa, ab, bb, determinant;
    uint byte, degenerate, reserved0, reserved1;
};

struct Parameters {
    uint columns, rows, image_width, cell_width;
    uint cell_height, glyph_count, foreground_threshold, background_threshold;
};

kernel void fit_background(device const uchar *image [[buffer(0)]],
                           device const float *alphas [[buffer(1)]],
                           device const Glyph *glyphs [[buffer(2)]],
                           device uchar *output [[buffer(3)]],
                           constant Parameters &params [[buffer(4)]],
                           uint cell [[thread_position_in_grid]]) {
    if (cell >= params.columns * params.rows) {
        return;
    }

    uint column = cell % params.columns;
    uint row = cell / params.columns;
    uint count = params.cell_width * params.cell_height;
    uint origin = (row * params.cell_height * params.image_width + column * params.cell_width) * 3;
    uint3 sum = uint3(0);
    uint3 minimum = uint3(255);
    uint3 maximum = uint3(0);
    uint sum_squared = 0;
    uint luminance = 0;
    for (uint y = 0; y < params.cell_height; ++y) {
        for (uint x = 0; x < params.cell_width; ++x) {
            uint offset = origin + (y * params.image_width + x) * 3;
            uint3 pixel = uint3(image[offset], image[offset + 1], image[offset + 2]);
            sum += pixel;
            minimum = min(minimum, pixel);
            maximum = max(maximum, pixel);
            sum_squared += pixel.x * pixel.x + pixel.y * pixel.y + pixel.z * pixel.z;
            luminance += (2126 * pixel.x + 7152 * pixel.y + 722 * pixel.z) / 10000;
        }
    }

    // Integer comparisons preserve threshold behavior without a floating divide.
    bool emit_foreground = luminance >= params.foreground_threshold * count;
    bool emit_background = luminance >= params.background_threshold * count;
    uchar3 average = uchar3(sum / count);
    uchar best_byte = ' ';
    uchar3 best_foreground = uchar3(0);
    uchar3 best_background = emit_background ? average : uchar3(0);
    uchar refinement = 0;
    if (emit_foreground && all(minimum == maximum)) {
        // A uniform patch needs only the cached reference tie-break, not a full
        // GPU glyph search. Its colors are known already.
        best_foreground = average;
        refinement = 3;
    } else if (emit_foreground) {
        float best_error = INFINITY;
        float second_error = INFINITY;
        bool best_rounding_risk = false;
        for (uint g = 0; g < params.glyph_count; ++g) {
            Glyph glyph = glyphs[g];
            uchar3 foreground = average;
            uchar3 background = average;
            float error = float(sum_squared);
            bool rounding_risk = false;
            if (glyph.degenerate) {
                for (uint c = 0; c < 3; ++c) {
                    float value = float(average[c]);
                    error += float(count) * value * value - 2.0f * value * float(sum[c]);
                }
            } else {
                float3 weighted = float3(0.0f);
                for (uint y = 0; y < params.cell_height; ++y) {
                    for (uint x = 0; x < params.cell_width; ++x) {
                        float alpha = alphas[g * count + y * params.cell_width + x];
                        if (alpha == 0.0f) {
                            continue;
                        }
                        uint offset = origin + (y * params.image_width + x) * 3;
                        weighted += alpha * float3(image[offset], image[offset + 1], image[offset + 2]);
                    }
                }
                for (uint c = 0; c < 3; ++c) {
                    float inverse = float(sum[c]) - weighted[c];
                    float fg = (weighted[c] * glyph.bb - inverse * glyph.ab) / glyph.determinant;
                    float bg = (inverse * glyph.aa - weighted[c] * glyph.ab) / glyph.determinant;
                    rounding_risk |= (fg > 0.0f && fg < 255.0f && abs(fract(fg) - 0.5f) < 0.002f);
                    rounding_risk |= (bg > 0.0f && bg < 255.0f && abs(fract(bg) - 0.5f) < 0.002f);
                    foreground[c] = uchar(floor(clamp(fg, 0.0f, 255.0f) + 0.5f));
                    background[c] = uchar(floor(clamp(bg, 0.0f, 255.0f) + 0.5f));
                    float f = float(foreground[c]);
                    float b = float(background[c]);
                    error += f * f * glyph.aa + 2.0f * f * b * glyph.ab + b * b * glyph.bb - 2.0f * (f * weighted[c] + b * inverse);
                }
            }
            if (error < best_error) {
                second_error = best_error;
                best_error = error;
                best_rounding_risk = rounding_risk;
                best_byte = uchar(glyph.byte);
                best_foreground = foreground;
                best_background = emit_background ? background : uchar3(0);
            } else if (error < second_error) {
                second_error = error;
            }
        }
        // Near ties are numerically sensitive, especially flat colors where a
        // hidden glyph becomes visible if backgrounds are suppressed. Mark these
        // for the compatibility reference. This is a conservative heuristic,
        // not a claim of bit-exact equivalence for every possible image.
        float margin = max(0.001f, float(sum_squared) * 0.00001f);
        bool ambiguous = best_rounding_risk || second_error - best_error <= margin;
        refinement = ambiguous ? 1 : 0;
    }

    uint offset = cell * 8;
    output[offset] = best_byte;
    for (uint c = 0; c < 3; ++c) {
        output[offset + 1 + c] = best_foreground[c];
        output[offset + 4 + c] = best_background[c];
    }
    output[offset + 7] = refinement;
}
