//! Listas de escolha (as opções de imagem da aba Conectar): o texto dos itens
//! de um `ComboBox` e a ida e volta entre o índice escolhido e o valor. Puras.
//!
//! Cada opção é um enum com `ALL` (a ordem da lista) e `label` (o texto): a
//! lista sai dele, sem repetir os textos no form.

/// Os itens de um `ComboBox`, na ordem de `all`.
pub fn items<T: Copy>(all: &[T], label: fn(T) -> &'static str) -> Vec<&'static str> {
    all.iter().map(|v| label(*v)).collect()
}

/// O índice de `value` em `all` (0, o primeiro, se não estiver).
pub fn index_of<T: PartialEq>(all: &[T], value: &T) -> i64 {
    all.iter().position(|v| v == value).unwrap_or(0) as i64
}

/// O valor no índice escolhido (o padrão do tipo fora da lista: -1, nada escolhido).
pub fn at<T: Copy + Default>(all: &[T], index: i64) -> T {
    usize::try_from(index).ok().and_then(|i| all.get(i).copied()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    enum Cor {
        #[default]
        Azul,
        Verde,
    }

    const TODAS: [Cor; 2] = [Cor::Azul, Cor::Verde];

    fn nome(c: Cor) -> &'static str {
        match c {
            Cor::Azul => "Azul",
            Cor::Verde => "Verde; claro",
        }
    }

    #[test]
    fn items_follow_the_order() {
        assert_eq!(items(&TODAS, nome), ["Azul", "Verde; claro"]);
    }

    #[test]
    fn index_and_value_round_trip() {
        assert_eq!(index_of(&TODAS, &Cor::Verde), 1);
        assert_eq!(at(&TODAS, 1), Cor::Verde);
    }

    #[test]
    fn out_of_range_falls_back_to_the_default() {
        assert_eq!(at(&TODAS, -1), Cor::Azul);
        assert_eq!(at(&TODAS, 9), Cor::Azul);
    }
}
