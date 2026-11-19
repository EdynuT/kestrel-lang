# Kestrel — Roteiro até a 1.0

**Meta da 1.0:** o compilador do Kestrel (`ksc`) escrito **em Kestrel** compila a si mesmo (*bootstrap*).

A máquina virtual (`ksvm`) continua em Rust, porque não existe hardware ternário para rodar o código direto. É a mesma divisão do Java: o `javac` é escrito em Java, e a JVM em C++.

```
hoje:  programa.ks ──[ksc em Rust]────────────────────► programa.ksb ──[ksvm]──► roda
1.0:   programa.ks ──[ksc.ksb (Kestrel) rodando na ksvm]──► programa.ksb ──[ksvm]──► roda
```

## Versões

| Versão | Conteúdo | Estado |
|---|---|---|
| 0.1 | Compilador (`ksc`) e VM (`ksvm`) em Rust, de ponta a ponta | ✅ |
| 0.2 | `int`, cast `(T) x`, `print`/`printl`/`printf`, `to_text`, `final`, impressão de buffers, tabela de caracteres ampliada | ✅ |
| 0.3 | **Controle e arrays:** `if`/`else`, `while`, `==` `<` `>` (e `!=` `<=` `>=`), enums, buffers a partir de 0 (qualquer tamanho), literal de array `[1, 2, 3]` | |
| 0.4 | **Textos e E/S:** comparar, cortar, montar e converter textos, classificar caracteres, ler arquivo e stdin, escrever bytes, argumentos de linha de comando, `exit` | |
| 0.5 | **Dados:** `struct` (só dados), buffers de structs, buffer que cresce | |
| 0.6 | **Classes:** `class` com métodos, `private`/`public`, interfaces (sem herança), anotações (`@impl`, `@test`, `@deprecated`) | |
| 0.7 | **Ferramentas:** módulos (`use`), traceback com linha do `.ks`, `assert`, biblioteca padrão escrita em Kestrel (lista, hash map, hash set, funções de texto) | |
| 0.8 | `ksc.ks`: lexer e parser | |
| 0.9 | `ksc.ks`: verificação de tipos e de posse, gerador de código e escrita do `.ksb` | |
| **1.0** | Bootstrap: o teste abaixo passa | |

A biblioteca padrão da 0.7 e o `ksc.ks` (0.8 e 0.9) são escritos como **aulas**: o código Kestrel é escrito pelo autor da linguagem, com explicação e revisão.

**Atenção:** cada recurso que entra antes da 1.0 precisa ser implementado **duas vezes**: em Rust, e depois no `ksc.ks`. A linguagem até a 1.0 deve ter só o necessário.

## Decisões de design para as próximas versões

- **`struct` × `class`:** `struct` guarda só dados e é copiável. `class` tem métodos e `private`/`public`, e tem dono (`own`).
- **Sem herança:** reaproveitamento por composição (uma classe tem outra como campo) e por interfaces, como em Go e Rust. Herança conflita com a posse (quem é dono da parte herdada? o que o `burn` libera?).
- **Sem construtor especial:** o objeto é criado preenchendo os campos: `Token { kind: 1, start: 0, len: 4 }`.
- **Anotações** são sempre minúsculas (`@impl`) e só são conferidas na compilação, como em Java.
- **Sem toques de assembly** (removidos na 0.3: `@R`, `add d, s;`, `tri cmp3`), porque eram só outra forma de escrever. Se um dia for preciso acesso real às instruções da ksvm, a ideia é um bloco `asm { }` isolado, depois da 1.0.
- **Dois tipos de buffer:** o simétrico (`claim N`, `N` ímpar, índices `-k..+k`) continua sendo o padrão. O buffer a partir de 0 aceita qualquer tamanho, com índices `0..N-1`, para listas, pilhas e textos em construção. A sintaxe exata é decidida na 0.3.
- **Hash map e hash set** são biblioteca escrita em Kestrel, não parte da linguagem.
- **Orientação a dados:** a árvore do compilador fica num buffer de nós ligados por índice, sem ponteiros dentro de dados.

## Como cada parte do `ksc.ks` é validada

Cada parte é comparada com o `ksc` em Rust, que serve de referência. Para os mesmos programas de teste, as duas versões precisam produzir a mesma saída: a mesma lista de tokens, a mesma árvore, os mesmos erros e o mesmo `.ksb`, byte a byte.

## Teste da 1.0 (ponto fixo)

1. O `ksc` em Rust compila `ksc.ks` e gera `ksc1.ksb`.
2. `ksvm ksc1.ksb ksc.ks` gera `ksc2.ksb`.
3. `ksvm ksc2.ksb ksc.ks` gera `ksc3.ksb`.
4. `ksc2.ksb` e `ksc3.ksb` são **idênticos byte a byte**.

Depois da 1.0, o `ksc` em Rust continua no repositório como referência e para iniciar o bootstrap numa máquina nova.

## Versões e tags

Cada versão lançada atualiza o `version` do `Cargo.toml` e recebe uma tag `vX.Y.Z`.
