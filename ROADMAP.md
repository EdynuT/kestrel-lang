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
| 0.2 | `int`, cast `(T) x`, `print`/`printl`/`printf`, `toText`, `final`, impressão de buffers, tabela de caracteres ampliada | ✅ |
| 0.3 | **Controle e arrays:** `if`/`else`, `while`, `for`, comparações (`== != < > <= >=`), `+=`/`++`, enums, `match`, arrays a partir de 0 (`x[]: int`) e simétricos (`sym x[]: int`), literal `{1, 2, 3}` | ✅ |
| 0.4 | **Textos, E/S e erros:** comparar, cortar, montar e converter textos, classificar caracteres, ler arquivo e stdin, escrever bytes, argumentos de linha de comando, `exit`, contas que falham disparam Trap e `on NaT` trata | ✅ |
| 0.5 | **Dados:** `struct` (só dados), arrays de qualquer tipo (inclusive structs), arrays que mudam de tamanho (`append`, `pop`, `insert`, `remove`, `resize`, `clear`, `replace`) | ✅ |
| 0.6 | **Classes:** `class` com métodos, `private`/`public`, interfaces (sem herança), anotações (`@impl`, `@test`, `@deprecated`) | |
| 0.7 | **Ferramentas:** módulos (`use`), seção de símbolos e tabela de linhas no `.ksb`, documentação `/** */`, executável único, traceback com linha do `.ks`, `assert`, biblioteca padrão mínima escrita em Kestrel (hash map, hash set, funções de texto) | |
| 0.8 | `ksc.ks`: lexer e parser | |
| 0.9 | `ksc.ks`: verificação de tipos e de posse, gerador de código e escrita do `.ksb` | |
| **1.0** | Bootstrap: o teste abaixo passa | |

A biblioteca padrão da 0.7 e o `ksc.ks` (0.8 e 0.9) são escritos como **aulas**: o código Kestrel é escrito pelo autor da linguagem, com explicação e revisão.

**Atenção:** cada recurso que entra antes da 1.0 precisa ser implementado **duas vezes**: em Rust, e depois no `ksc.ks`. A linguagem até a 1.0 deve ter só o necessário.

## Decisões de design para as próximas versões

- **`struct` × `class`:** `struct` guarda só dados e é copiável. `class` tem métodos e `private`/`public`, e tem dono (`own`).
- **Classes (0.6), decidido em 2026-11-19:**
  - O método recebe o objeto como `self`, escrito nos parâmetros: `lent self` (usa) ou `own self` (consome). Método sem `self` é "static" e se chama pelo nome da classe: `Conta.nova()`.
  - Campos e métodos são `private` por padrão; o que for visível fora leva `public`.
  - O contrato se chama `interface`. A classe declara que o cumpre com `@impl(Interface)` em cima dela, e o compilador confere que todos os métodos existem com a assinatura certa.
  - Cada método que cumpre a interface leva `@override` (obrigatório, como um `@Override` do Java que não pode faltar). Um `@override` que não cumpre nada é erro (pega erros de digitação).
  - Campos podem ter dono (arrays, outros objetos); `burn c` libera tudo o que está dentro.
  - `Conta { ... }` só cria o objeto dentro da própria classe; fora dela se usa um método static (`Conta.nova()`), o "construtor" sem palavra especial.
  - `c.metodo()` empresta `c` (`lent self`); um método `own self` consome o objeto. `print(c)` mostra os campos.
  - Arrays de objetos ficam para depois das interfaces.
  - Um objeto de classe tem dono, como um array: `burn`, `give`, `lend` e `return`, e esquecer é erro de compilação.
- **Sem herança:** reaproveitamento por composição (uma classe tem outra como campo) e por interfaces, como em Go e Rust. Herança conflita com a posse (quem é dono da parte herdada? o que o `burn` libera?).
- **Sem construtor especial:** o objeto é criado preenchendo os campos: `Token { kind: 1, start: 0, len: 4 }`.
- **Anotações** são sempre minúsculas (`@impl`) e só são conferidas na compilação, como em Java.
- **Sem toques de assembly** (removidos na 0.3: `@R`, `add d, s;`, `tri cmp3`), porque eram só outra forma de escrever. Se um dia for preciso acesso real às instruções da ksvm, a ideia é um bloco `asm { }` isolado, depois da 1.0.
- **Dois tipos de array** (0.3): `x[]: int` com índices `0..N-1` e `sym x[]: int` com índices `-k..+k`. Os colchetes ficam no nome; `{1, 2, 3}` é o literal; parâmetros usam `lent`/`own` como modificador.
- **Hash map e hash set** são biblioteca escrita em Kestrel, não parte da linguagem.
- **Lista = o próprio array** (0.5): como não há genéricos, uma `List` na biblioteca teria que ser escrita uma vez por tipo. Por isso o array tem `append`, `pop` etc. embutidos, para qualquer tipo de elemento.
- **Primitivas × biblioteca:** só vira instrução da ksvm o que a linguagem não consegue fazer sozinha (arquivos, stdin, argumentos, hora, número aleatório). Todo o resto (math, logging, lista, hash map, funções de texto) é biblioteca escrita em Kestrel.
- **A biblioteca padrão é feita em duas fases:**
  - **Antes da 1.0:** só o que o `ksc.ks` usa: hash map, montar textos, ler arquivo, erros com mensagem.
  - **Depois da 1.0:** math completo, logging, data e hora, números aleatórios e o que mais aparecer.
- **Erros:** uma conta que falha (overflow, divisão por zero, `toInt` inválido) dispara Trap com o motivo, e `on NaT` trata o erro na própria linha. `try`/`catch` completo, com o erro subindo de um proc para quem chamou, fica para depois da 1.0, se ainda fizer falta.
- **`.ksb` legível, como o `.class` do Java (0.7), ideia de 2026-11-19:**
  - **Seção de símbolos:** o `.ksb` guarda tudo o que é público, com os tipos: `proc area(lent self) -> real`, campos, `struct`, `enum`, `class` e `interface`. Com ela, `use geometria;` confere os tipos lendo só o `geometria.ksb`, sem o `.ks`, como o `javac` lê um `.class`.
  - **Tabela de linhas:** cada instrução aponta para a linha do `.ks` que a gerou. Serve para o traceback e para o editor.
  - **Documentação, como no Java:** `//` continua sendo comentário comum, que some na compilação. Um comentário `/** ... */` logo acima de um item público é a documentação dele: vai para a seção de símbolos do `.ksb` e aparece no hover do editor e na visualização, como o Javadoc. Não existe `/* */` comum.
  - **Documentação do módulo:** o primeiro `/** */` do arquivo, seguido de uma linha em branco, documenta o módulo inteiro (como o `package-info.java`). Passar o mouse em `use time;` mostra esse texto.
  - **Formato da documentação:** Markdown (títulos, negrito, listas, tabelas, blocos de código), como no Java 23 (JEP 467), no Rust e no Kotlin, porque o hover do VSCode já é Markdown. `{@link alvo}` vira um link para a definição. No fim vêm as tags `@since`, `@author`, `@param`, `@return` e `@deprecated`.

    ```
    /**
     * Data e hora: ler o relógio, somar dias e formatar datas.
     *
     * - **Fuso:** sempre UTC.
     * - Para intervalos, veja {@link duracao}.
     *
     * @since 1.0
     * @author Edynu
     */
    ```
- **Onde cada coisa mora (0.7), ideia de 2026-11-19:**

  | Camada | Exemplo | Onde mora |
  |---|---|---|
  | Primitivas | `print`, `len`, `append`, `readFile` | Instruções do `ksvm`, como o `libjvm` do Java |
  | Biblioteca padrão | hash map, funções de texto | `.ksb` embutidos dentro do `ksc`/`ksvm` (`include_bytes!`), como o `lib/modules` do JDK |
  | Bibliotecas externas | a lib de outra pessoa | Arquivos `.ksb`, como os `.jar` |

  `use texto;` procura primeiro dentro do executável e depois nos arquivos do projeto.
- **Executável único (0.7):** o `.ksb` continua sendo o formato de biblioteca e de módulo, mas para distribuir o `ksc` gera um executável só: uma cópia do `ksvm` com os `.ksb` usados grudados no fim, e uma marca dizendo onde começam. Quem recebe roda com dois cliques, sem instalar uma VM (diferente do `.jar`, que exige o JRE). Vai junto o *runtime*, não o compilador, como no Go. Depois da 1.0, o próprio `ksc.ksb` vira o executável `ksc` assim.
- **Cabeçalho de licença:** todo arquivo da biblioteca padrão e do `ksc.ks` começa com
  ```
  // Copyright (c) 2026 Edynu
  // SPDX-License-Identifier: MIT
  ```
  Como é `//`, não vira documentação.
  - **Visualização só leitura:** abrir um `.ksb` (ou dar F12 num item importado) mostra as assinaturas, com a documentação e o corpo como `/* compilado */`, e o bytecode (disassembly) logo abaixo. No topo vai um aviso: `// Gerado a partir de geometria.ksb pelo ksc. Somente leitura.`
  - Opcionalmente, o `.ksb` pode embutir o `.ks` original (como o `-sources.jar`). Aí a visualização mostra o código exato.
- **Orientação a dados:** a árvore do compilador fica num buffer de nós ligados por índice, sem ponteiros dentro de dados.

## Como cada parte do `ksc.ks` é validada

Cada parte é comparada com o `ksc` em Rust, que serve de referência. Para os mesmos programas de teste, as duas versões precisam produzir a mesma saída: a mesma lista de tokens, a mesma árvore, os mesmos erros e o mesmo `.ksb`, byte a byte.

## Teste da 1.0 (ponto fixo)

1. O `ksc` em Rust compila `ksc.ks` e gera `ksc1.ksb`.
2. `ksvm ksc1.ksb ksc.ks` gera `ksc2.ksb`.
3. `ksvm ksc2.ksb ksc.ks` gera `ksc3.ksb`.
4. `ksc2.ksb` e `ksc3.ksb` são **idênticos byte a byte**.

Depois da 1.0, o `ksc` em Rust continua no repositório como referência e para iniciar o bootstrap numa máquina nova.

## Depois da 1.0

- Biblioteca padrão completa: math, logging, data e hora, números aleatórios.
- `try`/`catch` completo, se o `on NaT` não bastar.
- Bloco `asm { }` para acesso direto às instruções da ksvm.
- Compilação AOT (bytecode para código de máquina, como o GraalVM), se o executável único com o `ksvm` ficar lento.
- Descompilador completo: reconstruir `if`/`while`/`tri` a partir dos saltos do bytecode, como o FernFlower.

## Versões e tags

Cada versão lançada atualiza o `version` do `Cargo.toml` e recebe uma tag `vX.Y.Z`.
