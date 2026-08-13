use kiss3d::glamx::Vec2;

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        #[cfg(target_arch = "wasm32")]
        // paths are weird in macros, so we have to re-export this to the consume so that they can call it.
        $crate::web_sys::console::log_1(&::std::string::String::from(format!($($arg)*)).into());
        #[cfg(not(target_arch = "wasm32"))]
        println!($($arg)*);
    };
}

// this is the rectangles height and width is displayed in the game world
#[derive(Debug, PartialEq, Clone, Copy)]
pub struct GameRectangle {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl GameRectangle {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        GameRectangle {
            x,
            y,
            width,
            height,
        }
    }

    pub fn get_half_extents_for_physics(&self, pixel_to_physics_scale: f32) -> Vec2 {
        Vec2::new(
            (self.width * 0.5) * pixel_to_physics_scale,
            (self.height * 0.5) * pixel_to_physics_scale,
        )
    }

    pub fn get_position(&self) -> Vec2 {
        Vec2 {
            x: self.x,
            y: self.y,
        }
    }
}
