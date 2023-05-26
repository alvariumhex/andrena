pub struct Vertex {
    pub id: String,
    pub content: String,
}

pub struct Edge {
    pub from: String,
    pub to: String,
}

pub struct Graph {
    pub vertices: Vec<Vertex>,
    pub edges: Vec<Edge>,
}

impl Graph {
    pub fn new() -> Graph {
        Graph {
            vertices: Vec::new(),
            edges: Vec::new(),
        }
    }

    pub fn add_or_replace_vertex(&mut self, id: String, content: String) {
        if let Some(_) = self.get_vertex(&id) {
            self.vertices.retain(|v| v.id != id);
        }

        self.vertices.push(Vertex { id, content });
    }

    pub fn add_edge(&mut self, from: String, to: String) {
        if self.get_edge(&from, &to).is_some() {
            return;
        }

        self.edges.push(Edge { from, to });
    }

    pub fn get_vertex(&self, id: &str) -> Option<&Vertex> {
        self.vertices.iter().find(|v| v.id == id)
    }

    fn get_edge(&self, from: &str, to: &str) -> Option<&Edge> {
        self.edges.iter().find(|e| e.from == from && e.to == to)
    }

    pub fn get_edges_from(&self, from: &str) -> Vec<&Edge> {
        self.edges.iter().filter(|e| e.from == from).collect()
    }

    pub fn get_edges_to(&self, to: &str) -> Vec<&Edge> {
        self.edges.iter().filter(|e| e.to == to).collect()
    }

    /// Returns a Graphviz representation of the graph.
    pub fn to_dot(&self) -> String {
        let mut dot = String::new();

        dot.push_str("digraph {\n");

        for vertex in &self.vertices {
            dot.push_str(&format!(
                "    {} [label=\"{}\"];\n",
                vertex.id, vertex.content
            ));
        }

        for edge in &self.edges {
            dot.push_str(&format!("    {} -> {};\n", edge.from, edge.to));
        }

        dot.push_str("}");

        dot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graph_using_dot() {
        let mut graph = Graph::new();

        graph.add_or_replace_vertex("1".to_string(), "Vertex 1".to_string());
        graph.add_or_replace_vertex("2".to_string(), "Vertex 2".to_string());

        graph.add_edge("1".to_string(), "2".to_string());

        let dot = graph.to_dot();

        assert_eq!(
            dot,
            "digraph {\n    1 [label=\"Vertex 1\"];\n    2 [label=\"Vertex 2\"];\n    1 -> 2;\n}"
        );
    }

    #[test]
    fn test_vert() {
        let mut graph = Graph::new();

        graph.add_or_replace_vertex("1".to_string(), "Vertex 1".to_string());
        graph.add_or_replace_vertex("2".to_string(), "Vertex 2".to_string());

        graph.add_edge("1".to_string(), "2".to_string());

        let vert = graph.get_vertex("1").unwrap();

        assert_eq!(vert.id, "1");
        assert_eq!(vert.content, "Vertex 1");

        graph.add_or_replace_vertex("1".to_string(), "Vertex 1.1".to_string());

        let vert = graph.get_vertex("1").unwrap();

        assert_eq!(vert.content, "Vertex 1.1");
    }
}
