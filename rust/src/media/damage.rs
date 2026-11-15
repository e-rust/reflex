//! Partes da tela que mudaram desde o último quadro codificado.
//!
//! A DXGI diz quais retângulos mudaram em cada imagem nova. No caminho da CPU
//! (placa de vídeo de software: VM, servidor) copiar e converter só esses
//! pedaços, em vez da tela inteira, é a maior parte do custo por quadro.

/// Retângulo em pixels (`right`/`bottom` exclusivos).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

impl Region {
    pub fn is_empty(&self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }

    pub fn area(&self) -> u64 {
        if self.is_empty() {
            0
        } else {
            u64::from(self.right - self.left) * u64::from(self.bottom - self.top)
        }
    }

    fn union(&self, o: &Region) -> Region {
        Region { left: self.left.min(o.left), top: self.top.min(o.top), right: self.right.max(o.right), bottom: self.bottom.max(o.bottom) }
    }

    /// Dentro de `w`×`h` e com cantos pares (o 4:2:0 converte blocos 2×2).
    pub fn even_within(&self, w: u32, h: u32) -> Region {
        Region { left: self.left.min(w) & !1, top: self.top.min(h) & !1, right: self.right.min(w).next_multiple_of(2).min(w & !1), bottom: self.bottom.min(h).next_multiple_of(2).min(h & !1) }
    }
}

/// O que mudou desde o último quadro codificado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Damage {
    /// Tudo (início, troca de captura, a DXGI não disse).
    Full,
    /// Só estes retângulos (vazio = nada mudou).
    Rects(Vec<Region>),
}

impl Default for Damage {
    fn default() -> Self {
        Damage::Rects(Vec::new())
    }
}

/// Acima disso a lista vira um retângulo só (cada um custa uma cópia).
const MAX_RECTS: usize = 24;

impl Damage {
    /// Soma uma imagem nova: `rects` = o que ela mudou (`None` = não se sabe).
    pub fn add(&mut self, rects: Option<&[Region]>) {
        let Damage::Rects(list) = self else { return };
        let Some(rects) = rects else {
            *self = Damage::Full;
            return;
        };
        list.extend(rects.iter().filter(|r| !r.is_empty()));
        if list.len() > MAX_RECTS {
            let all = list.iter().skip(1).fold(list[0], |a, r| a.union(r));
            *list = vec![all];
        }
    }

    /// Pega o acumulado e recomeça do zero.
    pub fn take(&mut self) -> Damage {
        std::mem::take(self)
    }

    /// Os retângulos pra uma imagem `w`×`h`, prontos pro 4:2:0 (cantos pares,
    /// dentro da imagem); `None` = tudo. Se cobrem quase a tela, tudo.
    pub fn regions(&self, w: u32, h: u32) -> Option<Vec<Region>> {
        let Damage::Rects(list) = self else { return None };
        let out: Vec<Region> = list.iter().map(|r| r.even_within(w, h)).filter(|r| !r.is_empty()).collect();
        let total: u64 = out.iter().map(Region::area).sum();
        (total * 10 < u64::from(w) * u64::from(h) * 9).then_some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(left: u32, top: u32, right: u32, bottom: u32) -> Region {
        Region { left, top, right, bottom }
    }

    #[test]
    fn corners_become_even_and_stay_inside() {
        assert_eq!(r(3, 5, 7, 9).even_within(100, 100), r(2, 4, 8, 10));
        assert_eq!(r(90, 90, 120, 130).even_within(101, 99), r(90, 90, 100, 98));
    }

    #[test]
    fn unknown_change_means_everything() {
        let mut d = Damage::default();
        d.add(Some(&[r(0, 0, 10, 10)]));
        d.add(None);
        assert_eq!(d, Damage::Full);
        d.add(Some(&[r(0, 0, 2, 2)]));
        assert_eq!(d.take(), Damage::Full);
        assert_eq!(d, Damage::default());
    }

    #[test]
    fn many_rects_collapse_into_their_bounds() {
        let mut d = Damage::default();
        for i in 0..30 {
            d.add(Some(&[r(i * 10, i * 2, i * 10 + 4, i * 2 + 4)]));
        }
        let Damage::Rects(list) = d else { panic!("virou tudo") };
        assert!(list.len() <= MAX_RECTS, "{}", list.len());
        assert_eq!(list.iter().skip(1).fold(list[0], |a, x| a.union(x)), r(0, 0, 294, 62));
    }

    #[test]
    fn regions_of_a_small_change_but_all_of_a_big_one() {
        let mut d = Damage::default();
        assert_eq!(d.regions(100, 100), Some(vec![]));
        d.add(Some(&[r(10, 10, 20, 20), r(0, 0, 0, 5)]));
        assert_eq!(d.regions(100, 100), Some(vec![r(10, 10, 20, 20)]));
        d.add(Some(&[r(0, 0, 100, 95)]));
        assert_eq!(d.regions(100, 100), None);
        assert_eq!(Damage::Full.regions(100, 100), None);
    }
}
