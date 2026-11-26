# Kestrel — Roteiro até a 1.0

**Meta da 1.0:** o compilador do Kestrel (`ksc`) escrito **em Kestrel** compila a si mesmo (*bootstrap*).

A máquina virtual (`ksvm`) continua em Rust, porque não existe hardware ternário para rodar o código direto. É a mesma divisão do Java: o `javac` é escrito em Java, e a JVM em C++.

```
hoje:  programa.kstl ──[ksc em Rust]────────────────────► programa.kstb ──[ksvm]──► roda
1.0:   programa.kstl ──[ksc.kstb (Kestrel) rodando na ksvm]──► programa.kstb ──[ksvm]──► roda
```

## Versões

| Versão | Conteúdo | Estado |
|---|---|---|
| 0.1 | Compilador (`ksc`) e VM (`ksvm`) em Rust, de ponta a ponta | ✅ |
| 0.2 | `int`, cast `(T) x`, `print`/`printl`/`printf`, `toText`, `final`, impressão de buffers, tabela de caracteres ampliada | ✅ |
| 0.3 | **Controle e arrays:** `if`/`else`, `while`, `for`, comparações (`== != < > <= >=`), `+=`/`++`, enums, `match`, arrays a partir de 0 (`x[]: int`) e simétricos (`sym x[]: int`), literal `{1, 2, 3}` | ✅ |
| 0.4 | **Textos, E/S e erros:** comparar, cortar, montar e converter textos, classificar caracteres, ler arquivo e stdin, escrever bytes, argumentos de linha de comando, `exit`, contas que falham disparam Trap e `on NaT` trata | ✅ |
| 0.5 | **Dados:** `struct` (só dados), arrays de qualquer tipo (inclusive structs), arrays que mudam de tamanho (`append`, `pop`, `insert`, `remove`, `resize`, `clear`, `replace`) | ✅ |
| 0.6 | **Classes:** `class` com métodos, `private`/`public`, interfaces (sem herança), arrays de objetos, interfaces com dono, árvores e objetos opcionais (`T?`), anotações (`@impl`, `@override`, `@test`, `@deprecated`), `assert` e `ksc --test` | ✅ |
| 0.7 | **Ferramentas:** módulos (`use`), seção de símbolos e tabela de linhas no `.kstb`, documentação `/* */`, executável único, traceback com linha do `.kstl`, biblioteca padrão mínima escrita em Kestrel (hash map, hash set, funções de texto) | ✅ |
| 0.8 | `ksc.kstl`: lexer e parser | |
| 0.9 | `ksc.kstl`: verificação de tipos e de posse, gerador de código e escrita do `.kstb` | |
| **1.0** | Bootstrap: o teste abaixo passa | |

A biblioteca padrão da 0.7 e o `ksc.kstl` (0.8 e 0.9) são escritos como **aulas**: o código Kestrel é escrito pelo autor da linguagem, com explicação e revisão.

**Atenção:** cada recurso que entra antes da 1.0 precisa ser implementado **duas vezes**: em Rust, e depois no `ksc.kstl`. A linguagem até a 1.0 deve ter só o necessário.

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
  - Arrays de objetos (2026-11-20): o array é dono dos objetos, que entram com `give` e saem com `pop`/`remove` (que agora devolve o elemento). `v[i]` fica no array (emprestado). Sem elementos vazios: `claim`, `resize` e `clear` não valem para eles, e trocar `v[i] = ...` é erro. `burn v` libera cada objeto.
- **Antes do compilador (0.7), decidido em 2026-11-21:**
  - Null-safety como em Kotlin, para todos os tipos: um tipo sem `?` nunca é null; `int?`, `text?`, `No?` podem ser. Escreve-se `null`, `x == null`, `x != null` (sai o `isNaT`; `NaT` só no `on NaT`). O compilador prova o "não é null" (if, `&&`/`||`, `return` cedo, `assert`, atribuição) e exige a prova antes de usar. `a?.b` e `a?.m()`. Declarar sem valor exige `?`. `toInt`, `toReal` e `input` devolvem `?` em vez de disparar Trap.
  - Empréstimo com `&`, como em Rust (pedido do autor: `lend`/`lent` confundiam): `c: &Conta` e `&v[]: int` emprestam, `&self` empresta o objeto; sem `&` o parâmetro recebe a posse (`c: Conta`, `self`), e quem chama escreve `&c` ou `give c`. As palavras `lent`, `lend` e `own` saíram (dão erro com a forma nova).
  - Coringas (`? extends T`, `? super T`): considerados e **descartados** (2026-11-21). Com uma cópia do código por tipo, `proc f<T extends Forma>(fs: &List<T>)` chamado como `f<Circulo>(&lista)` faz o mesmo, e uma lista mista já é `List<Forma>`. Coringas obrigariam o compilador a deduzir o tipo pelo argumento (a única dedução da linguagem), criando cópias no meio da checagem.
  - Construtor com o nome da classe, como no Java, mas sem `new`: `public proc Conta(saldo: real) -> Conta`, chamado `Conta(10.0)` (genérico: `List<int>()`). Um por classe (não há sobrecarga); outras formas de criar são métodos estáticos com nome. Interfaces não têm construtor.
  - Feito em 2026-11-21: devolver empréstimo (`-> &T`, do primeiro parâmetro `&`), `x: &int` (copiado na entrada, escrito de volta na saída), `Object` automático (`toString`, `hashCode`, `equals`; `==` em objetos chama `equals`, `print` usa `toString`), `Comparable` sem `<T>` (como Swift/Rust: compara com a mesma classe; `<` em objetos e textos), `x ?: padrão`. `Comparable<T>` com interfaces genéricas ficou de fora: em `T extends Comparable<T>` o `T` aparece na própria restrição, o que a checagem dos genéricos não resolve hoje. `Rc` e ponteiros crus ficam para depois da 1.0.
  - Licença trocada de MIT para GPL v3 (ou posterior) em 2026-11-21.
  - Build (2026-11-21), sem arquivo de projeto (a estrutura das pastas diz tudo, como Java/C sem Gradle/CMake): `src/` (código; `Main.kstl` é o principal e os `use` partem daqui), `lib/` (bibliotecas de terceiros como `.kstb`, como a pasta de `.jar` soltos), `build/obj/` (um `.kstb` por arquivo, cache) e `build/` (o executável). Pastas em minúscula, arquivos em PascalCase (`use jogo.Mapa;` lê `src/jogo/Mapa.kstl`). A biblioteca padrão é `std`, por assunto (`std.text`, `std.collections`), embutida no `ksc` e sempre com `use` (nada entra sozinho). Subcomandos `ksc build`, `ksc run`, `ksc test`, `ksc new` para projetos; `ksc arquivo.kstl` com flags para o uso avulso. O `.kstb` guarda as declarações do arquivo, então uma lib funciona sem o `.kstl`. O compilador para no primeiro erro (até a 1.0).
  - Erros com `try`/`catch` entram na 0.7 (antes ficavam para depois da 1.0), no modelo do Java: `proc ler(p: text) -> Config throws NotFound, ParseError`, conferido pelo compilador (quem chama trata ou declara `throws`), chamadas sem marca. Lança-se um objeto de uma classe com `@impl(Error)` (`message(&self) -> text`); `catch e: NotFound` pega uma classe, `catch e: Error` todas. As falhas da VM (overflow, divisão por zero, índice, `assert`, null) viram o erro `Trap`, que não precisa de `throws` (como `RuntimeException`) e que `catch e: Trap` pega; sem `catch`, param o programa como hoje. Quando um erro sai de um bloco, o compilador libera sozinho o que ainda tem dono ali; o erro pego tem dono como qualquer objeto. Feito em 2026-11-21 (SPEC §8.17): uma tabela de tratadores por função no `.kstb` (versão 3), como a do `.class` do Java, e a instrução `THROW`.
  - Extensões trocadas (2026-11-23, pedido do autor): o código-fonte passa de `.ks` para `.kstl` e o compilado de `.ksb` para `.kstb` (com `--kstb`), porque o `.ks` já é do kOS, o mod de programação do Kerbal Space Program. O `ksc` só aceita as novas.
  - Versionamento (2026-11-23, decisão do autor): **a partir da 1.0, retrocompatibilidade sempre**, como no Java. Um programa que compila numa versão compila e roda igual em todas as seguintes; as versões novas só acrescentam, nunca removem nem mudam o sentido do que existe. Por isso a 1.0 já sai com o básico completo e bem pensado, para não precisar tirar nada depois. Até a 1.0 a linguagem ainda pode mudar (como `lend`/`lent` → `&` e `.ks` → `.kstl`). Sem calendário: as versões saem no ritmo do autor, quando ele quiser. Descartado o modelo do Python, que dá mais liberdade para mudar a linguagem mas quebra o código feito nas versões antigas (como na passagem do 2 para o 3).
  - Sobrecarga (2026-11-22, pedido do autor): procs e métodos com o mesmo nome e parâmetros de tipos diferentes; a chamada escolhe pela versão exata, ou pela única em que os argumentos cabem (SPEC §8.13.2). Também precisa ser feita no `ksc.kstl`.
  - Funções embutidas na biblioteca padrão (2026-11-22, pedido do autor: nada escondido dentro do executável), como no Java: `public native proc printl(values: ...);` em `std.io.Console`, e da mesma forma `std.io.Files`, `std.os.Process`, `std.text.Texts`, `std.text.Chars`, `std.debug.Debug`. `printl(...)` sem `use` é erro, e a mensagem diz o `use`. Só `len`, as operações de array, `assert` e `isNaT` continuam globais (SPEC §8.6, §8.16.2).
  - Código de máquina (2026-11-21): o `ksc` em Rust ganha um gerador de código nativo antes da 1.0, para o `ksc build` dar um ELF com instruções x86-64 em vez da cópia do `ksvm` com o bytecode (hoje o executável já roda sem nada instalado, mas por dentro ainda é a VM). Só Linux x86-64 por enquanto; Windows fica para depois. O `.kstb` continua como objeto e para o `ksvm`. Versão a definir, depois do `try`/`catch`. Escolhido em 2026-11-21: assembler e linker próprios (sem `as`/`ld`). Etapa 1 feita no mesmo dia: o ELF é uma cópia do `ksvm` com um segmento a mais; saltos e chamadas são nativos, as outras instruções chamam o runtime (1,6× mais rápido que a VM). Etapa 2 feita no mesmo dia: mover valores, contas e comparações de `int` e desvios em x86 direto (11× mais rápido que a VM num laço com `fib`). Etapa 3 feita no mesmo dia: `/` e `%`, e a pilha adiada (variáveis, constantes e o último resultado só vão para a pilha da VM quando preciso): 76× mais rápido que a VM num laço com `%`. Etapa 4 feita no mesmo dia: chamadas e retornos sem o runtime (as variáveis de todas as chamadas numa área só, que a VM também usa), e `v[i]`, `v[i] = x` e `len(v)` direto no heap: `fib(30)` 30× e um laço com array 51× mais rápidos que a VM. Etapa 5 feita no mesmo dia: campos de objetos e structs (`obj.campo`, `ps[i].x`), constantes de texto e de real, e `== null`: um laço com objetos e um array de structs 40× mais rápido que a VM. Etapa 6 feita no mesmo dia: reais e operações de texto que não criam texto chamam direto `krt_math` (reais 7× mais rápidos que a VM). Criar textos (`+`, `toText`) continua pelo caminho completo do runtime, por causa do coletor de lixo.
  - Arquivos e `use` (2026-11-21), modelo do `gcc` com o `.h` gerado sozinho: `use Geometry;` / `use Shapes.Circle;`, nomes de arquivo PascalCase, itens de outro arquivo sempre com prefixo (`Geometry.area()`), o item com o nome do arquivo vale pelo nome do arquivo (`List<int>()` de `List.kstl`), só o `public` sai do arquivo. Ciclos permitidos (cada arquivo compila lendo só as declarações dos outros, como os `.h` do C, sem escrevê-los à mão). Um `.kstb` por arquivo em `build/`, recompilado só quando ele ou a parte pública do que ele usa muda; o ligador junta os objetos e escreve as chamadas por interface. O resultado é o executável (cópia do `ksvm` com o programa); `--kstb` dá o `.kstb`. O arquivo principal não pode ser usado (como o `package main` do Go). Comparado: o Rust compila um crate inteiro junto (ciclos só dentro do crate), o Go proíbe ciclos.
  - Trit 0 numa condição continua disparando Trap (para tratar os três casos existe `tri`).
  - Variável emprestada: `lent atual: No? = lend lista;` percorre listas e árvores com laços; o empréstimo vale até o último uso, e enquanto isso o dono não pode ser queimado, entregue, esvaziado nem emprestado. O `burn` de uma lista ligada também virou laço.
  - Genéricos, como no Java na escrita: `class Box<T>`, `proc max<T extends Comparable>(a: T, b: T) -> T`, o tipo sempre escrito (`List<int>.new()`, `max<int>(3, 7)`), coringas `? extends T` e `? super T`. `T` pode ser qualquer tipo, inclusive `int` (não há caixas como `Integer`) e objetos com dono. Por dentro, um código por tipo usado (como C++/Rust), não *type erasure*: os valores têm tamanhos diferentes.
  - Sem herança até a 1.0 (decidido em 2026-11-21; depois da 1.0 o autor decide, ou se alguém pedir). No lugar: métodos padrão em interfaces (como no Java 8) e `Object` como uma interface que todo tipo cumpre sozinho (`equals`, `hashCode`, `toString`), então um `<T>` sem `extends` já compara e imprime. Todo código nativo (biblioteca, built-ins) tem nomes em inglês.
  - Tipos recursivos (2026-11-20), escolhido o modelo do Kotlin/Swift: `filhos[]: No` (árvores) e campos opcionais `proximo: No?` (vazio = NaT). O compilador obriga a conferir com `isNaT` antes de usar, e a conferência se perde quando algo pode ter esvaziado o campo. `give n.proximo` tira o objeto; atribuir só a um `T?` vazio (senão Trap). `burn` usa uma função escondida por classe, então libera árvores de qualquer profundidade.
  - Voltar da interface para a classe (2026-11-20): `f is Circulo` (como em Kotlin, não `==`) e `match f { Circulo => ...; }`; onde provado, o próprio `f` vira `Circulo` (sem nome novo por ramo, para não haver dois nomes para o mesmo objeto).
  - Interfaces com dono (2026-11-20): variáveis, campos, `own`, retorno e arrays de uma interface guardam objetos de classes diferentes; `burn` libera conforme a classe.
  - Um objeto de classe tem dono, como um array: `burn`, `give`, `lend` e `return`, e esquecer é erro de compilação.
- **Sem herança:** reaproveitamento por composição (uma classe tem outra como campo) e por interfaces, como em Go e Rust. Herança conflita com a posse (quem é dono da parte herdada? o que o `burn` libera?).
- **Construtor (2026-11-21):** o método estático com o nome da classe (`Conta(...)`), que cria o objeto preenchendo os campos: `Token { kind: 1, start: 0, len: 4 }`.
- **Anotações** são sempre minúsculas (`@impl`) e só são conferidas na compilação, como em Java.
- **Sem toques de assembly** (removidos na 0.3: `@R`, `add d, s;`, `tri cmp3`), porque eram só outra forma de escrever. Se um dia for preciso acesso real às instruções da ksvm, a ideia é um bloco `asm { }` isolado, depois da 1.0.
- **Dois tipos de array** (0.3): `x[]: int` com índices `0..N-1` e `sym x[]: int` com índices `-k..+k`. Os colchetes ficam no nome; `{1, 2, 3}` é o literal; parâmetros usam `lent`/`own` como modificador.
- **Hash map e hash set** são biblioteca escrita em Kestrel, não parte da linguagem.
- **Lista = o próprio array** (0.5): como não há genéricos, uma `List` na biblioteca teria que ser escrita uma vez por tipo. Por isso o array tem `append`, `pop` etc. embutidos, para qualquer tipo de elemento.
- **Primitivas × biblioteca:** só vira instrução da ksvm o que a linguagem não consegue fazer sozinha (arquivos, stdin, argumentos, hora, número aleatório). Todo o resto (math, logging, lista, hash map, funções de texto) é biblioteca escrita em Kestrel.
- **A biblioteca padrão é feita em duas fases:**
  - **Antes da 1.0:** só o que o `ksc.kstl` usa: hash map, montar textos, ler arquivo, erros com mensagem.
  - **Depois da 1.0:** math completo, logging, data e hora, números aleatórios e o que mais aparecer.
- **Erros:** uma conta que falha (overflow, divisão por zero, `toInt` inválido) dispara Trap com o motivo, e `on NaT` trata o erro na própria linha. O erro que sobe de um proc para quem chamou é o `try`/`catch` da 0.7 (veja as decisões acima).
- **`.kstb` legível, como o `.class` do Java (0.7), ideia de 2026-11-19:**
  - **Seção de símbolos:** o `.kstb` guarda tudo o que é público, com os tipos: `proc area(lent self) -> real`, campos, `struct`, `enum`, `class` e `interface`. Com ela, `use geometria;` confere os tipos lendo só o `geometria.kstb`, sem o `.kstl`, como o `javac` lê um `.class`.
  - **Tabela de linhas:** cada instrução aponta para a linha do `.kstl` que a gerou. Serve para o traceback e para o editor. Feito em 2026-11-22: cada função do `.kstb` guarda o arquivo e a tabela de linhas, e o Trap mostra as chamadas com arquivo e linha (também no executável nativo).
  - **Documentação, como no Java:** `//` continua sendo comentário comum, que some na compilação. Um comentário `/* ... */` logo acima de um item público é a documentação dele: vai para a seção de símbolos do `.kstb` e aparece no hover do editor e na visualização, como o Javadoc.
  - **Documentação do módulo:** o primeiro `/* */` do arquivo, seguido de uma linha em branco, documenta o módulo inteiro (como o `package-info.java`). Passar o mouse em `use time;` mostra esse texto.
  - **Formato da documentação:** Markdown (títulos, negrito, listas, tabelas, blocos de código), como no Java 23 (JEP 467), no Rust e no Kotlin, porque o hover do VSCode já é Markdown. `{@link alvo}` vira um link para a definição. No fim vêm as tags `@since`, `@author`, `@param`, `@return`, `@see` (um item relacionado, como `@see List.add`) e `@deprecated`. Feito em 2026-11-22: o `ksc` liga cada `/* */` ao item, método ou campo logo abaixo (e o primeiro do arquivo, seguido de linha em branco, ao arquivo); `ksc doc` dá o texto do nome numa posição, e a extensão mostra no hover. Uma biblioteca em `lib/` leva a sua documentação dentro do `.kstb` (nas declarações). No mesmo dia: `@see` vira link, e a extensão ganhou Go to Definition, Go to Implementations, Go to Super Implementation (a interface de um `@override`) e a hierarquia de tipos, pelo nome (`ksc nav`). Find References, Call Hierarchy e Go to Type Definition precisam dos tipos: ficam para um servidor de linguagem (`ksc lsp`). Ainda sem `{@link}` virar link.

    ```
    /*
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
  | Biblioteca padrão | hash map, funções de texto | `.kstb` embutidos dentro do `ksc`/`ksvm` (`include_bytes!`), como o `lib/modules` do JDK |
  | Bibliotecas externas | a lib de outra pessoa | Arquivos `.kstb`, como os `.jar` |

  `use texto;` procura primeiro dentro do executável e depois nos arquivos do projeto.
- **Executável único (0.7):** o `.kstb` continua sendo o formato de biblioteca e de módulo, mas para distribuir o `ksc` gera um executável só: uma cópia do `ksvm` com os `.kstb` usados grudados no fim, e uma marca dizendo onde começam. Quem recebe roda com dois cliques, sem instalar uma VM (diferente do `.jar`, que exige o JRE). Vai junto o *runtime*, não o compilador, como no Go. Depois da 1.0, o próprio `ksc.kstb` vira o executável `ksc` assim.
- **Cabeçalho de licença:** todo arquivo da biblioteca padrão e do `ksc.kstl` começa com
  ```
  // Copyright (c) 2026 Edynu
  // SPDX-License-Identifier: GPL-3.0-or-later
  ```
  Como é `//`, não vira documentação.
  - **Visualização só leitura:** abrir um `.kstb` (ou dar F12 num item importado) mostra as assinaturas, com a documentação e o corpo como `/* compilado */`, e o bytecode (disassembly) logo abaixo. No topo vai um aviso: `// Gerado a partir de geometria.kstb pelo ksc. Somente leitura.`
  - Opcionalmente, o `.kstb` pode embutir o `.kstl` original (como o `-sources.jar`). Aí a visualização mostra o código exato.
- **Orientação a dados:** a árvore do compilador fica num buffer de nós ligados por índice, sem ponteiros dentro de dados.

## Como cada parte do `ksc.kstl` é validada

Cada parte é comparada com o `ksc` em Rust, que serve de referência. Para os mesmos programas de teste, as duas versões precisam produzir a mesma saída: a mesma lista de tokens, a mesma árvore, os mesmos erros e o mesmo `.kstb`, byte a byte.

## Teste da 1.0 (ponto fixo)

1. O `ksc` em Rust compila `ksc.kstl` e gera `ksc1.kstb`.
2. `ksvm ksc1.kstb ksc.kstl` gera `ksc2.kstb`.
3. `ksvm ksc2.kstb ksc.kstl` gera `ksc3.kstb`.
4. `ksc2.kstb` e `ksc3.kstb` são **idênticos byte a byte**.

Depois da 1.0, o `ksc` em Rust continua no repositório como referência e para iniciar o bootstrap numa máquina nova.

## Revisão antes da 1.0

Como a partir da 1.0 nada pode ser removido nem mudar de sentido (veja "Versionamento" em "Decisões de design"), tudo o que parecer estranho, incompleto ou que limite o uso entra aqui quando aparecer. Antes de marcar a 1.0, cada item é resolvido ou fica como está por uma decisão consciente, registrada aqui.

- **Reatribuir depois de `give`** (2026-11-23): `left = Node(...)` depois de `give left` é erro (SPEC §6.2), e por isso o `moreBinary` do parser em Kestrel teve que ser recursivo em vez de um laço. Depois da 1.0 a regra só poderia ficar mais permissiva, nunca mais rígida.
- **Lista com valores iniciais** (2026-11-23): não há como criar uma `List` já com elementos (`List<Node>([x])`, `List<Node> { x }`): é preciso criar vazia e chamar `add`. A sintaxe que entrar será permanente.
- **Valor padrão em parâmetros** (2026-11-23): `proc padLeft(value: text, width: int, padding: text = " ")` dá erro. Decidir se entra (e como convive com a sobrecarga) ou se fica de fora para sempre.
- **Arquivos com `\r\n`** (2026-11-23): o `\r` não está na tabela de trytes (SPEC §4), então um programa em Kestrel não consegue ler um arquivo salvo no Windows; o `ksc` em Rust aceita. Afeta qualquer programa que leia texto.
- **Enum como texto** (2026-11-23): `"kind: " + kind` não funciona com um enum; o compilador em Kestrel precisou de um `symbol()` escrito à mão. Um `toString` automático para enums seria uma adição, mas o formato do texto fica para sempre.
- **Templates em qualquer texto** (2026-11-26): hoje `{x}` só é trocado pelo valor no `printf`; fora dele, `"{x}"` é o próprio texto. Interpolar em todo texto (`"Olá, {nome}"`) mudaria o sentido de código que já funciona, então a escolha é antes da 1.0: ou todo texto interpola (e `{` passa a ser escrito `\{`), ou fica reservado um prefixo (`$"Olá, {nome}"`, como o C#), que pode entrar depois sem quebrar nada.
- **Palavras reservadas para o futuro** (2026-11-26): uma palavra que vira reservada depois da 1.0 quebra quem já a usou como nome. As que podem ser úteis um dia ficam reservadas antes, como o Java fez com `const` e `goto` (a lista fica na SPEC, §11).
- **Nomes definitivos** (2026-11-23): confirmar os nomes que viram permanentes, como `throws`, `Trap`, `Error`, `claim`, `burn`, `give`, `walk`, `tri` e `sym`.

## Depois da 1.0

- Biblioteca padrão completa: math, logging, data e hora, números aleatórios.
- Bloco `asm { }` para acesso direto às instruções da ksvm.
- Descompilador completo: reconstruir `if`/`while`/`tri` a partir dos saltos do bytecode, como o FernFlower.
- Recursos vindos do Kotlin, todos hoje erros de compilação (por isso entram sem quebrar nada):
  - **`match` como expressão:** `label: text = match kind { TokenKind.Plus => "+", else => "?" };`.
  - **`match` sem assunto:** `match { x < 0 => ..., x == 0 => ..., else => ... }`, no lugar de uma cadeia de `if`/`else if`.
- **Operador ternário do Java** (escolhido no lugar do `if` como expressão do Kotlin): `condição ? seSim : seNão`, como em `Texts.toText(count) + (count == 1 ? " error" : " errors")`. Hoje é erro de compilação, então entra sem quebrar nada. Pontos a decidir quando entrar: dentro das chaves do `printf` o `:` já começa o formato (`{x:t}`), então lá o ternário vai entre parênteses (`{(x > 0 ? x : 0)}`); a precedência é a mais baixa de todas, como no Java (`a || b ? c : d` é `(a || b) ? c : d`), e ele agrupa pela direita (`a ? b : c ? d : e` é `a ? b : (c ? d : e)`); e um `+` ou `-` sozinho antes do `:` é o trit (`ok ? + : -`), o que põe o `:` na lista de delimitadores do trit.
  - **Argumentos com nome:** `Diagnostic(description: "...", start: a, end: b)`. Cuidado: quando entrar, o nome de cada parâmetro da biblioteca padrão vira parte da promessa de compatibilidade (renomear quebra quem chama). Combina com o valor padrão em parâmetros (veja "Revisão antes da 1.0").
  - **Funções de extensão:** `proc text.repeat(&self, count: int) -> text`, chamada como `" ".repeat(3)`. Cuidado: decidir o que acontece quando a biblioteca padrão ganha um método com o mesmo nome de uma extensão (no Kotlin o método da classe vence sem aviso, e o comportamento muda em silêncio).
  - **`sealed interface`:** as classes que cumprem a interface são só as do mesmo arquivo, e um `match` com `is` sobre ela precisa cobrir todas, como o `match` de enum. Um nó da árvore de sintaxe viraria uma classe por tipo, com campos com nome, em vez de `kind` e uma lista de filhos. A palavra `sealed` já fica reservada.
- Bibliotecas separadas do programa, no caminho do Java (`.jar`): o `ksvm` carrega vários `.kstb` quando o programa roda, em vez de tudo ir para um executável só. O caminho de `.so`/DLL nativo fica de fora: os genéricos geram uma cópia por tipo usado, que uma biblioteca já compilada não tem como trazer, e o executável nativo precisaria da ligação dinâmica do Linux (PLT/GOT). A forma dos objetos e das chamadas dentro de um `.kstb` passaria a ser uma promessa (ABI) entre versões.

## Versões e tags

Cada versão lançada atualiza o `version` do `Cargo.toml` e recebe uma tag `vX.Y.Z`.
