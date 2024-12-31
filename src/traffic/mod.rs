use crate::input::PlayerInput;
use crate::traffic::network::{NodeIndex, TrafficNetwork};
use bevy::color::palettes::basic::GREEN;
use bevy::color::palettes::css::DARK_BLUE;
use bevy::prelude::*;

mod network;

pub struct TrafficPlugin;

impl Plugin for TrafficPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(TrafficNetwork::new())
            .add_systems(Update, (place_traffic_node, draw_network));
    }
}

pub fn place_traffic_node(
    player_input: Res<PlayerInput>,
    mut network: ResMut<TrafficNetwork>,
    mut editing_node: Local<Option<NodeIndex>>,
    mut previous_node_position: Local<Vec3>,
) {
    if let Some(position) = player_input.world_cursor_position {
        if player_input.tap {
            let start_node = if let Some(node_index) = *editing_node {
                if let Some(start_node) = &network.nodes[node_index] {
                    *previous_node_position = start_node.position;
                }
                node_index
            } else {
                // Create the starting node on the current position
                let new_index = network.create_node(position, Vec3::NEG_Z);
                *previous_node_position = position;
                new_index
            };
            // Create the editing node, which will be moved around until the next tap
            let target_node = network.create_node(position, Vec3::NEG_Z);
            *editing_node = Some(target_node);
            // Create the segment between the two nodes
            network.add_segment(start_node, target_node);
        } else {
            // Move the editing node to the current position
            if let Some(node) = *editing_node {
                if let Some(editing_node) = &mut network.nodes[node] {
                    editing_node.position = position;
                    editing_node.forward = (position - *previous_node_position).normalize_or_zero();
                }
            }
        }
    }
}

pub fn draw_network(network: Res<TrafficNetwork>, mut gizmos: Gizmos) {
    for node in network.nodes.iter().flatten() {
        gizmos.sphere(node.position, 0.12, GREEN);
        gizmos.arrow(node.position, node.position + node.forward, GREEN);
        for segment in network.outgoing_segments(node) {
            let segment = network.segments[segment].clone().unwrap();
            let end_node = network.nodes[segment.target_node].clone().unwrap();
            let start = node.position;
            let end = end_node.position;
            // TODO: There's probably a better way to calculate the control points
            let control_a = start + node.get_junction_direction_vector(&segment.source_dir);
            let control_b = end + end_node.get_junction_direction_vector(&segment.target_dir);
            let curve = CubicBezier::new([[start, control_a, control_b, end]])
                .to_curve()
                .unwrap();
            let resolution = 100 * curve.segments().len();
            gizmos.linestrip(
                curve
                    .iter_positions(resolution)
                    .map(|pos| pos + Vec3::Y * 0.02),
                DARK_BLUE,
            );
        }
    }
}
