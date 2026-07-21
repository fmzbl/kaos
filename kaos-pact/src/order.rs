//! The Pact — the secret-society layer.
//!
//! The Pact is an organizational structure for those wishing to perform magic,
//! and it exploits the device of a graded structure.
//! kaos's agents are **all members of this order**. The Pact
//! owns the roster, performs the **routing** (which adept is assigned a sigil), and
//! runs the **grade economy** (who is elevated, who is humbled) — i.e. it is the
//! learning mixture-of-experts router, and the [`Egregore`] is its shared mind.

use crate::adept::Adept;
use crate::egregore::Egregore;
use crate::grade::Grade;
use crate::ray::Ray;

/// The order: a roster of sworn adepts, a Supreme Magus, and the collective mind.
pub struct Pact {
    /// The Supreme Magus (0°) — the orchestrator-self; routes but does not toil.
    pub supreme: Adept,
    /// The sworn membership.
    pub members: Vec<Adept>,
    /// The shared mind.
    pub egregore: Egregore,
}

impl Pact {
    /// Convene the default Pact: one adept sworn to each of the seven worker rays,
    /// each under a magical motto in the chaos-magick tradition (a Frater/Soror
    /// name). Two extra Red adepts — kaos's principal ray is the best-staffed.
    pub fn convene() -> Pact {
        use Grade::{Adept as G2, Initiate};
        use Ray::*;
        let members = vec![
            Adept::sworn("Frater Tenebrae", Black, Initiate),
            Adept::sworn("Soror Argentum", Blue, Initiate),
            Adept::sworn("Frater Stokastikos", Red, G2),
            Adept::sworn("Soror Bellona", Red, Initiate),
            Adept::sworn("Frater Sol Niger", Yellow, Initiate),
            Adept::sworn("Soror Concordia", Green, Initiate),
            Adept::sworn("Frater Hermeticus", Orange, G2),
            Adept::sworn("Soror Genetrix", Purple, Initiate),
        ];
        Pact {
            supreme: Adept::sworn("Frater Kaos", Ray::Octarine, Grade::SupremeMagus),
            members,
            egregore: Egregore::new(),
        }
    }

    /// **Route** a task to the fittest sworn adept for its ray. The Supreme Magus
    /// reads each member's fitness (home-ray competence × grade × temperament) and
    /// assigns the strongest. Returns the index into `members`.
    pub fn route(&self, task_ray: Ray) -> usize {
        let mut best = 0;
        let mut best_fit = f64::MIN;
        for (i, m) in self.members.iter().enumerate() {
            let f = m.fitness(task_ray);
            if f > best_fit {
                best_fit = f;
                best = i;
            }
        }
        best
    }

    /// **Convene a conclave** — the `k` fittest adepts for a ray, for quorum work.
    /// Returns their indices, fittest first.
    pub fn conclave(&self, task_ray: Ray, k: usize) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.members.len()).collect();
        idx.sort_by(|&a, &b| {
            self.members[b]
                .fitness(task_ray)
                .partial_cmp(&self.members[a].fitness(task_ray))
                .unwrap()
        });
        idx.truncate(k.max(1).min(self.members.len()));
        idx
    }

    /// The roster, fittest-graded first, as lines for the TUI.
    pub fn roster(&self) -> Vec<String> {
        let mut lines = vec![self.supreme.epithet()];
        let mut ms: Vec<&Adept> = self.members.iter().collect();
        ms.sort_by(|a, b| b.grade.cmp(&a.grade).then(a.home.name().cmp(b.home.name())));
        for m in ms {
            lines.push(m.epithet());
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_to_home_ray_specialist() {
        let pact = Pact::convene();
        let i = pact.route(Ray::Orange);
        assert_eq!(pact.members[i].home, Ray::Orange);
        let j = pact.route(Ray::Black);
        assert_eq!(pact.members[j].home, Ray::Black);
    }

    #[test]
    fn conclave_is_fittest_first() {
        let pact = Pact::convene();
        let c = pact.conclave(Ray::Red, 3);
        assert_eq!(c.len(), 3);
        // The Red home-ray adepts should lead the conclave for a Red task.
        assert_eq!(pact.members[c[0]].home, Ray::Red);
    }

    #[test]
    fn convene_swears_every_worker_ray() {
        let pact = Pact::convene();
        for ray in Ray::worker_rays() {
            assert!(
                pact.members.iter().any(|m| m.home == ray),
                "missing ray {:?}",
                ray
            );
        }
    }
}
