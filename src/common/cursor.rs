use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

/// The flat top of an upright hexagonal prism, which a cursor ray can land on.
///
/// The footprint is a hexagon of circumradius `radius` centred on the entity's global translation,
/// with a vertex towards `-Z` to match the grid's tiles, and the surface stands `height` above that
/// translation. A tile is one of these lying on the ground; anything standing on a tile is one
/// raised by its own height.
#[derive(Component)]
#[require(Transform)]
pub struct CursorSurface {
    /// Circumradius of the hexagonal footprint.
    pub radius: f32,
    /// How far the surface stands above the entity's translation.
    pub height: f32,
}

/// Marks a [`CursorSurface`] as the ground itself, so a ray landing on it names a tile.
#[derive(Component, Default)]
pub struct TileSurface;

/// Where a cursor ray landed, and the tile that place belongs to.
#[derive(Debug, PartialEq)]
pub struct CursorHit {
    /// The point on the nearest surface the ray meets.
    pub point: Vec3,
    /// The nearest tile the ray meets, which is the one under `point` when it is on an object.
    pub tile: Option<CursorTile>,
}

/// A tile a cursor ray met, and the middle of it.
#[derive(Debug, PartialEq, Clone, Copy)]
pub struct CursorTile {
    /// The tile itself.
    pub entity: Entity,
    /// The middle of the tile's surface, which is the only place on it an edit may go.
    pub centre: Vec3,
}

/// Casts a ray against every [`CursorSurface`] in the world.
#[derive(SystemParam)]
pub struct CursorRayCast<'w, 's> {
    surfaces: Query<
        'w,
        's,
        (
            Entity,
            &'static CursorSurface,
            &'static GlobalTransform,
            Has<TileSurface>,
        ),
    >,
}

impl CursorRayCast<'_, '_> {
    /// The nearest surface `ray` meets, and the nearest tile it meets, or nothing.
    ///
    /// The two are found separately so that a ray stopped by a building still names the tile the
    /// building stands on.
    pub fn cast(&self, ray: Ray3d) -> Option<CursorHit> {
        let mut nearest: Option<(f32, Vec3)> = None;
        let mut nearest_tile: Option<(f32, CursorTile)> = None;

        for (entity, surface, transform, is_tile) in &self.surfaces {
            let Some(distance) = surface_distance(ray, surface, transform.translation()) else {
                continue;
            };
            if nearest.is_none_or(|(nearest, _)| distance < nearest) {
                nearest = Some((distance, ray.get_point(distance)));
            }
            if is_tile && nearest_tile.is_none_or(|(nearest, _)| distance < nearest) {
                let centre = transform.translation() + Vec3::Y * surface.height;
                nearest_tile = Some((distance, CursorTile { entity, centre }));
            }
        }

        nearest.map(|(_, point)| CursorHit {
            point,
            tile: nearest_tile.map(|(_, tile)| tile),
        })
    }
}

/// How far along `ray` the surface centred on `centre` is, if the ray lands on it at all.
fn surface_distance(ray: Ray3d, surface: &CursorSurface, centre: Vec3) -> Option<f32> {
    let top = centre.y + surface.height;
    let distance = ray.intersect_plane(Vec3::new(0., top, 0.), InfinitePlane3d::default())?;
    let point = ray.get_point(distance);
    let offset = point - Vec3::new(centre.x, top, centre.z);
    inside_hexagon(Vec2::new(offset.x, offset.z), surface.radius).then_some(distance)
}

/// Whether a point offset from a hexagon's centre falls inside it.
///
/// A hexagon is the overlap of three slabs, one per pair of opposite edges, each reaching an
/// apothem either side of the centre. These normals put a vertex towards `-Y`, which in the grid's
/// plane is the `-Z` the tiles point along.
fn inside_hexagon(offset: Vec2, radius: f32) -> bool {
    let apothem = radius * HALF_ROOT_THREE;
    let edge_normals = [
        Vec2::X,
        Vec2::new(0.5, HALF_ROOT_THREE),
        Vec2::new(-0.5, HALF_ROOT_THREE),
    ];
    edge_normals
        .iter()
        .all(|normal| offset.dot(*normal).abs() <= apothem)
}

/// Half the square root of three: a hexagon's apothem as a fraction of its circumradius.
const HALF_ROOT_THREE: f32 = 0.866_025_4;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{headless_app, tick};
    use bevy::ecs::system::RunSystemOnce;

    const RADIUS: f32 = 10.;

    fn cast_app() -> App {
        headless_app()
    }

    fn spawn_surface(app: &mut App, centre: Vec3, height: f32) -> Entity {
        app.world_mut()
            .spawn((
                CursorSurface {
                    radius: RADIUS,
                    height,
                },
                Transform::from_translation(centre),
            ))
            .id()
    }

    fn spawn_tile(app: &mut App, centre: Vec3) -> Entity {
        let tile = spawn_surface(app, centre, 0.);
        app.world_mut().entity_mut(tile).insert(TileSurface);
        tile
    }

    fn straight_down(x: f32, z: f32) -> Ray3d {
        Ray3d::new(Vec3::new(x, 50., z), Dir3::NEG_Y)
    }

    fn cast(app: &mut App, ray: Ray3d) -> Option<CursorHit> {
        app.world_mut()
            .run_system_once(move |cast: CursorRayCast| cast.cast(ray))
            .expect("the cast runs as a system")
    }

    #[test]
    fn a_ray_lands_on_the_top_face_of_a_surface() {
        let mut app = cast_app();
        spawn_tile(&mut app, Vec3::ZERO);
        tick(&mut app);

        let hit = cast(&mut app, straight_down(0., 0.)).expect("the ray meets the tile");

        assert_eq!(hit.point, Vec3::ZERO);
    }

    #[test]
    fn a_ray_inside_the_circle_but_past_the_flat_lands_on_nothing() {
        let mut app = cast_app();
        spawn_tile(&mut app, Vec3::ZERO);
        tick(&mut app);

        let hit = cast(&mut app, straight_down(RADIUS * 0.95, 0.));

        assert_eq!(hit, None);
    }

    #[test]
    fn a_surface_standing_above_zero_is_hit_at_its_own_height() {
        let mut app = cast_app();
        spawn_surface(&mut app, Vec3::new(0., 4., 0.), 2.);
        tick(&mut app);

        let hit = cast(&mut app, straight_down(0., 0.)).expect("the ray meets the surface");

        assert_eq!(hit.point.y, 6.);
    }

    #[test]
    fn the_nearer_of_two_stacked_surfaces_takes_the_hit() {
        let mut app = cast_app();
        spawn_tile(&mut app, Vec3::ZERO);
        spawn_surface(&mut app, Vec3::ZERO, 3.);
        tick(&mut app);

        let hit = cast(&mut app, straight_down(0., 0.)).expect("the ray meets both surfaces");

        assert_eq!(hit.point.y, 3.);
    }

    #[test]
    fn an_object_standing_on_a_tile_is_still_named_by_that_tile() {
        let mut app = cast_app();
        let tile = spawn_tile(&mut app, Vec3::ZERO);
        spawn_surface(&mut app, Vec3::ZERO, 3.);
        tick(&mut app);

        let hit = cast(&mut app, straight_down(0., 0.)).expect("the ray meets the object");

        assert_eq!(hit.point.y, 3.);
        assert_eq!(hit.tile.map(|hit| hit.entity), Some(tile));
    }

    #[test]
    fn a_hit_names_the_middle_of_the_tile_it_belongs_to() {
        let mut app = cast_app();
        let centre = Vec3::new(3., 0., 1.);
        spawn_tile(&mut app, centre);
        tick(&mut app);

        let hit = cast(&mut app, straight_down(0., 0.)).expect("the ray meets the tile");

        assert_eq!(hit.tile.map(|hit| hit.centre), Some(centre));
    }

    #[test]
    fn a_ray_pointing_away_from_a_surface_lands_on_nothing() {
        let mut app = cast_app();
        spawn_tile(&mut app, Vec3::ZERO);
        tick(&mut app);

        let hit = cast(&mut app, Ray3d::new(Vec3::new(0., 50., 0.), Dir3::Y));

        assert_eq!(hit, None);
    }

    #[test]
    fn a_ray_meeting_no_surface_at_all_lands_on_nothing() {
        let mut app = cast_app();
        tick(&mut app);

        assert_eq!(cast(&mut app, straight_down(0., 0.)), None);
    }

    #[test]
    fn a_ray_landing_on_an_object_beside_the_grid_names_no_tile() {
        let mut app = cast_app();
        spawn_surface(&mut app, Vec3::new(100., 0., 0.), 1.);
        tick(&mut app);

        let hit = cast(&mut app, straight_down(100., 0.)).expect("the ray meets the object");

        assert_eq!(hit.tile, None);
    }

    #[test]
    fn a_point_on_the_flat_of_a_hexagon_is_inside_it() {
        assert!(inside_hexagon(Vec2::new(RADIUS * 0.85, 0.), RADIUS));
    }

    #[test]
    fn a_point_past_the_flat_of_a_hexagon_is_outside_it() {
        assert!(!inside_hexagon(Vec2::new(RADIUS * 0.9, 0.), RADIUS));
    }

    #[test]
    fn a_point_at_the_vertex_of_a_hexagon_is_inside_it() {
        assert!(inside_hexagon(Vec2::new(0., -RADIUS * 0.99), RADIUS));
    }
}
