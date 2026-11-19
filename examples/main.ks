struct Aluno {
    nome: text;
    nota1: real;
    nota2: real;
}

proc media(aluno: Aluno) -> real {
    media: real = (aluno.nota1 + aluno.nota2) / 2.0;
    return media;
}

proc main() {
    nome[]: text = {"Ana", "Bia"};
    printl(nome);

    turma[]: Aluno = claim 3;
    turma[0] = Aluno {nome: "Ana", nota1: 6.0, nota2: 4.5};
    turma[1] = Aluno {nome: "Bia", nota1: 8.5, nota2: 7.8};
    printf("%s\n", turma[0]);

    printf("A media de {turma[0].nome} é {media(turma[0])}\n");
    printf("A media de {turma[1].nome} é {media(turma[1])}\n");
    
    burn nome;
    burn turma;
}
