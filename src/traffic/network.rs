use bevy::prelude::*;

pub type NodeIndex = usize;
pub type SegmentIndex = usize;

/// The network of traffic nodes and segments, represented as a graph.
#[derive(Resource)]
pub struct TrafficNetwork {
    pub nodes: Vec<Option<TrafficNode>>,
    pub segments: Vec<Option<TrafficSegment>>,
}

/// A node in the traffic network, representing a junction between two or more roads.
#[derive(Clone)]
pub struct TrafficNode {
    /// The position of the node in the world.
    pub position: Vec3,
    /// A direction vector representing the forward direction of the node. Used alongside each
    /// segment's `JunctionDirection` to calculate the direction of the road at the junction.
    pub forward: Vec3,
    /// The index of the first outgoing segment from this node. If present, this segment will
    /// contain an index to the next outgoing segment, forming a linked list.
    first_outgoing_segment: Option<SegmentIndex>,
}

/// A road segment connecting two traffic nodes.
#[derive(Clone)]
pub struct TrafficSegment {
    /// The index of the node where this segment ends.
    pub target_node: NodeIndex,
    /// The direction of the road segment at the source node.
    pub source_dir: JunctionDirection,
    /// The direction of the road segment at the target node.
    pub target_dir: JunctionDirection,
    /// The index of the next outgoing segment from the source node.
    next_outgoing_segment: Option<SegmentIndex>,
}

/// The direction of a road segment at a traffic node, which determines the type of junction.
///
/// For example, a segment with a `Forward` direction creates a T-junction with a segment with a
/// `Right` or `Left` direction.
#[derive(Clone)]
pub enum JunctionDirection {
    Forward,
    Backward,
    Left,
    Right,
}

/// An iterator over the outgoing segments of a traffic node.
pub struct OutgoingSegments<'graph> {
    graph: &'graph TrafficNetwork,
    current_segment_index: Option<SegmentIndex>,
}

impl TrafficNetwork {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            segments: Vec::new(),
        }
    }

    pub fn create_node(&mut self, position: Vec3, forward: Vec3) -> NodeIndex {
        let index = self.nodes.len();
        let traffic_node = TrafficNode::new(position, forward);
        self.nodes.push(Some(traffic_node));
        index
    }

    pub fn add_segment(&mut self, source_node: NodeIndex, target_node: NodeIndex) -> SegmentIndex {
        if let Some(node_data) = &mut self.nodes[source_node] {
            let index = self.segments.len();
            let segment = TrafficSegment {
                target_node,
                next_outgoing_segment: node_data.first_outgoing_segment,
                source_dir: JunctionDirection::Forward,
                target_dir: JunctionDirection::Backward,
            };
            node_data.first_outgoing_segment = Some(index);
            self.segments.push(Some(segment));
            index
        } else {
            panic!("Tried to add segment to non-existent node");
        }
    }

    pub fn outgoing_segments(&self, source: &TrafficNode) -> OutgoingSegments {
        OutgoingSegments {
            graph: self,
            current_segment_index: source.first_outgoing_segment,
        }
    }
}

impl Iterator for OutgoingSegments<'_> {
    type Item = SegmentIndex;

    fn next(&mut self) -> Option<SegmentIndex> {
        match self.current_segment_index {
            None => None,
            Some(segment_index) => {
                if let Some(segment) = &self.graph.segments[segment_index] {
                    self.current_segment_index = segment.next_outgoing_segment;
                } else {
                    self.current_segment_index = None;
                }
                Some(segment_index)
            }
        }
    }
}

impl TrafficNode {
    pub fn new(position: Vec3, forward: Vec3) -> Self {
        Self {
            first_outgoing_segment: None,
            position,
            forward,
        }
    }

    pub fn get_junction_direction_vector(&self, direction: &JunctionDirection) -> Vec3 {
        match direction {
            JunctionDirection::Forward => self.forward,
            JunctionDirection::Backward => -self.forward,
            JunctionDirection::Right => self.forward.cross(Vec3::Y),
            JunctionDirection::Left => self.forward.cross(Vec3::NEG_Y),
        }
    }
}
