// Marks the pixels of the outlined meshlets in a mask, which `draw_outline` rings.
#include "scene_raster.slang"

[shader("fragment")]
float raster_outline_fragment() : SV_Target
{
    return 1.0;
}
